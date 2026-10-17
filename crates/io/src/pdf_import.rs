//! PDF import, as Photoshop's File › Open on a PDF (its Import PDF dialog): one page rasterised
//! into a transparent `Layer 1` at the chosen resolution, colour mode and depth, cropped to one of
//! the page's boxes or to the bounding box of its marks. Pages are interpreted and rasterised by
//! `hayro` (pure Rust, on the CPU).
//!
//! What Photoshop 25.4 does was measured on synthetic PDFs (page boxes, rotation, the bounding box
//! of fills, strokes, text and images, device colour spaces, optional content, annotations) and
//! is followed here:
//!
//! * pixel size = round-half-up(box in points × ppi / 72) per axis; `/Rotate` turns the page,
//!   `/UserUnit` is ignored;
//! * a page box missing from the file falls back as the PDF specification says (Bleed, Trim and
//!   Art boxes to the crop box, the crop box to the media box), every box is cut to the media box;
//! * the bounding box is the geometric extent of every mark, painted or not (white fills, fully
//!   transparent fills and invisible text count), cut by the clip in force and by the crop box;
//!   strokes grow their path bounds by max(line width / 2, 4) + 2 points on each side; text uses a
//!   generous box around each glyph origin (see [`GLYPH_BOX`]); an empty page is its crop box;
//! * a one-page PDF whose only mark is a single image (a scan, a photo wrapped as a PDF) opens as
//!   that image: its own pixels on a `Background` layer, resolution from its placed width, Gray
//!   or RGB as the image is, every option ignored;
//! * the document is named after the file (one page: the stem; several: `stem-N.pdf`), has no
//!   embedded profile (the working space applies), and isn't tied to the file for Save;
//! * DeviceCMYK converts like Photoshop's U.S. Web Coated (SWOP) v2 (hayro's compact CMYK profile
//!   is within one level of it); DeviceGray differs: Photoshop reads it through Dot Gain 20 %,
//!   hayro linearly (a mid grey is 149 there and 128 here).
//!
//! Known gaps: Media Box shows nothing outside the crop box (hayro clips to it); `NoView`
//! annotations are drawn; non-embedded fonts other than the standard 14 fall back to hayro's
//! base-14 stand-ins; Anti-aliased off settles coverage and opacity under one half as out.

use std::sync::Arc;

use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageData, ImageDrawProps, InterpreterCache, InterpreterSettings, LumaData, SoftMask,
    interpret_page,
};
use hayro::hayro_syntax::page::{Page, Rotation};
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::kurbo::{Affine, BezPath, Rect as PtRect, Shape};
use hayro::vello_cpu::color::palette::css::TRANSPARENT;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{self, Pixmap, RenderContext};
use hayro::{RenderCache, RenderSettings, render_into};
use photocraft_cms::{Builtin, Intent, Transform};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Layer, LayerContent};
use photocraft_geom::Size;
use photocraft_raster::{Interrupt, Surface};

use crate::{ImportResult, IoError};

/// Largest side a page opens at; a larger page is scaled down to fit, with a warning.
pub const MAX_SIDE: u32 = 16_384;
/// Most pixels a page opens with (a Letter page at 1200 ppi is 135 MP); past it the page is
/// scaled down, with a warning.
pub const MAX_PAGE_PIXELS: u64 = 64_000_000;
/// The resolution the dialog starts at, as Photoshop's.
pub const DEFAULT_RESOLUTION: f32 = 300.0;
/// The resolution range the dialog accepts.
pub const MIN_RESOLUTION: f32 = 1.0;
pub const MAX_RESOLUTION: f32 = 30_000.0;
/// The error for a PDF that can't be read without its user password; the UI asks for one then.
pub const PASSWORD_REQUIRED: &str = "the PDF is protected by a password";
/// The error for a password the PDF doesn't accept.
pub const WRONG_PASSWORD: &str = "that isn't the PDF's password";

/// The box around one glyph, in glyph units (1000 per em), relative to its origin: left, bottom,
/// right, top. Photoshop's text bounds are not the ink but a generous box per glyph, as measured
/// on Helvetica and Times text ("H" at 48 pt → 166 × 85 pt; the right edge is always the last
/// glyph's origin + 2.41 em, whatever the glyph). Courier and embedded fonts measure differently
/// (their own font boxes); this one box stands for all of them.
pub const GLYPH_BOX: [f64; 4] = [-1050.0, -530.0, 2410.0, 1240.0];

/// `true` if `bytes` start a PDF file: the `%PDF-` marker within the first kilobyte (the
/// specification allows leading bytes before it, and files do carry a byte-order mark or a
/// shell line there).
pub fn is_pdf(bytes: &[u8]) -> bool {
    let head = bytes.get(..bytes.len().min(1024)).unwrap_or(bytes);
    head.windows(5).any(|w| w == b"%PDF-")
}

/// Photoshop's Crop To choice: which page box becomes the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CropTo {
    /// The smallest box around every mark on the page (the default).
    #[default]
    BoundingBox,
    MediaBox,
    CropBox,
    BleedBox,
    TrimBox,
    ArtBox,
}

impl CropTo {
    /// Every choice, in the dialog's order.
    pub const ALL: [CropTo; 6] = [CropTo::BoundingBox, CropTo::MediaBox, CropTo::CropBox, CropTo::BleedBox, CropTo::TrimBox, CropTo::ArtBox];

    /// The value in command params (`"crop":"boundingBox|mediaBox|cropBox|bleedBox|trimBox|artBox"`).
    pub fn id(self) -> &'static str {
        match self {
            CropTo::BoundingBox => "boundingBox",
            CropTo::MediaBox => "mediaBox",
            CropTo::CropBox => "cropBox",
            CropTo::BleedBox => "bleedBox",
            CropTo::TrimBox => "trimBox",
            CropTo::ArtBox => "artBox",
        }
    }

    /// The dialog label (English source string).
    pub fn label(self) -> &'static str {
        match self {
            CropTo::BoundingBox => "Bounding Box",
            CropTo::MediaBox => "Media Box",
            CropTo::CropBox => "Crop Box",
            CropTo::BleedBox => "Bleed Box",
            CropTo::TrimBox => "Trim Box",
            CropTo::ArtBox => "Art Box",
        }
    }

    /// The choice an id or label names, case-insensitively.
    pub fn from_id(s: &str) -> Option<CropTo> {
        let s = s.trim();
        CropTo::ALL.into_iter().find(|c| c.id().eq_ignore_ascii_case(s) || c.label().eq_ignore_ascii_case(s))
    }
}

/// What the Import PDF dialog asked for.
#[derive(Clone, PartialEq)]
pub struct PdfOpenOptions {
    /// The page to open, counted from 1.
    pub page: usize,
    pub crop: CropTo,
    /// Pixels per inch of the document.
    pub resolution: f32,
    /// RGB, Grayscale, CMYK or Lab (anything else opens as RGB).
    pub mode: ColorMode,
    /// 8 or 16 bits per channel (anything else opens as 8).
    pub depth: SampleType,
    pub anti_alias: bool,
    /// The dialog's Name field: the document name's stem; the file's stem when `None`.
    pub name: Option<String>,
    /// The dialog's Width and Height, in pixels, when the user changed them: the page is scaled
    /// to this size at `resolution`. `None` sizes the page from its box and the resolution.
    pub pixel_size: Option<(u32, u32)>,
    /// The user password of an encrypted PDF.
    pub password: Option<String>,
    /// The dialog's Images choice: open raster image number N of the file (counted through the
    /// pages from 1) instead of a page; `page` and the page options are ignored then.
    pub image: Option<usize>,
    /// Photoshop's Suppress Warnings: open without import notes.
    pub suppress_warnings: bool,
}

/// Prints every option but the password (logs and error messages carry `{opts:?}`).
impl std::fmt::Debug for PdfOpenOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfOpenOptions")
            .field("page", &self.page)
            .field("crop", &self.crop)
            .field("resolution", &self.resolution)
            .field("mode", &self.mode)
            .field("depth", &self.depth)
            .field("anti_alias", &self.anti_alias)
            .field("name", &self.name)
            .field("pixel_size", &self.pixel_size)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .field("image", &self.image)
            .field("suppress_warnings", &self.suppress_warnings)
            .finish()
    }
}

impl Default for PdfOpenOptions {
    fn default() -> Self {
        PdfOpenOptions {
            page: 1,
            crop: CropTo::BoundingBox,
            resolution: DEFAULT_RESOLUTION,
            mode: ColorMode::Rgb,
            depth: SampleType::U8,
            anti_alias: true,
            name: None,
            pixel_size: None,
            password: None,
            image: None,
            suppress_warnings: false,
        }
    }
}

/// One page's boxes, in PDF points (`[x0, y0, x1, y1]`, y up), every one cut to the media box
/// and the missing ones filled in as the specification says, plus its `/Rotate`.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfPage {
    pub media: [f32; 4],
    pub crop: [f32; 4],
    pub bleed: [f32; 4],
    pub trim: [f32; 4],
    pub art: [f32; 4],
    /// 0, 90, 180 or 270 degrees clockwise.
    pub rotation: u16,
}

impl PdfPage {
    /// The page box `crop` names (the Bounding Box choice is [`bounding_box`]'s business: here
    /// it is the crop box, which bounds it).
    pub fn box_for(&self, crop: CropTo) -> [f32; 4] {
        match crop {
            CropTo::MediaBox => self.media,
            CropTo::CropBox | CropTo::BoundingBox => self.crop,
            CropTo::BleedBox => self.bleed,
            CropTo::TrimBox => self.trim,
            CropTo::ArtBox => self.art,
        }
    }

    /// Width and height in points of box `b` as the page is displayed: a page rotated by 90 or
    /// 270 degrees swaps them.
    pub fn displayed_size(&self, b: [f32; 4]) -> (f32, f32) {
        let (w, h) = ((b[2] - b[0]).max(0.0), (b[3] - b[1]).max(0.0));
        if self.rotation == 90 || self.rotation == 270 { (h, w) } else { (w, h) }
    }
}

/// What the dialog needs to know about a file before any page is rasterised.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfInfo {
    pub pages: Vec<PdfPage>,
}

/// A page rendered small, straight-alpha RGBA, for the dialog's thumbnails.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Pixel size of a `(width, height)` in points at `ppi`: Photoshop rounds half up on each axis
/// (A4 at 300 ppi is 2480 × 3508; 100.5 pt at 72 ppi is 101 px).
pub fn pixel_size(pt: (f32, f32), ppi: f32) -> (u32, u32) {
    let px = |v: f32| ((v * ppi / 72.0) + 0.5).floor().clamp(1.0, 1.0e9) as u32;
    (px(pt.0), px(pt.1))
}

/// Would `bytes` open as a plain image (a one-page PDF drawing nothing but one raster image)?
/// Photoshop opens such a file like an image, without its Import PDF dialog.
pub fn opens_as_image(bytes: &[u8], password: Option<&str>) -> bool {
    let Ok(pdf) = load(bytes, password) else { return false };
    if pdf.pages().len() != 1 {
        return false;
    }
    let Some(page) = pdf.pages().first() else { return false };
    let m = marks(page, None);
    m.paths == 0 && m.glyphs == 0 && m.stencils == 0 && m.images.len() == 1
}

/// Does opening `bytes` need a user password?
pub fn needs_password(bytes: &[u8]) -> bool {
    matches!(Pdf::new(Arc::new(bytes.to_vec())), Err(LoadPdfError::Decryption(_)))
}

/// The pages of a PDF: boxes and rotation of each.
pub fn info(bytes: &[u8], password: Option<&str>) -> Result<PdfInfo, IoError> {
    let pdf = load(bytes, password)?;
    let pages = pdf.pages().iter().map(page_info).collect();
    Ok(PdfInfo { pages })
}

/// The bounding box of page `page` (from 1), in PDF points of the unrotated page: the smallest
/// box around every mark, cut by the crop box; the crop box itself when the page draws nothing.
pub fn bounding_box(bytes: &[u8], page: usize, password: Option<&str>) -> Result<[f32; 4], IoError> {
    let pdf = load(bytes, password)?;
    let p = page_of(&pdf, page)?;
    let info = page_info(p);
    Ok(marks(p, None).bounds_within(info.crop))
}

/// Page `page` (from 1) rendered to fit `max_side` pixels on its longer side, on nothing (the
/// dialog paints the paper).
pub fn thumbnail(bytes: &[u8], page: usize, max_side: u32, password: Option<&str>) -> Result<Thumbnail, IoError> {
    let pdf = load(bytes, password)?;
    let p = page_of(&pdf, page)?;
    let info = page_info(p);
    let (dw, dh) = info.displayed_size(info.crop);
    let k = max_side.clamp(1, 2048) as f32 / dw.max(dh).max(1.0e-3);
    let side = |v: f32| ((v * k).round() as u32).clamp(1, 2048);
    let (w, h) = (side(dw), side(dh));
    let rgba = render_page(p, info.crop, info.rotation, (w, h), true);
    Ok(Thumbnail { width: w, height: h, rgba })
}

/// Opens one page of a PDF as a document (see the module docs). `name` is the file name.
pub fn import_pdf(name: &str, bytes: &[u8], opts: &PdfOpenOptions, ctl: &Interrupt) -> Result<ImportResult, IoError> {
    let mut r = import_pdf_inner(name, bytes, opts, ctl)?;
    if opts.suppress_warnings {
        r.warnings.clear();
    }
    Ok(r)
}

fn import_pdf_inner(name: &str, bytes: &[u8], opts: &PdfOpenOptions, ctl: &Interrupt) -> Result<ImportResult, IoError> {
    if let Some(index) = opts.image {
        return import_image(name, bytes, index, opts.name.as_deref(), opts.password.as_deref(), ctl);
    }
    ctl.check().map_err(|_| IoError::Cancelled)?;
    let pdf = load(bytes, opts.password.as_deref())?;
    let count = pdf.pages().len();
    let page = page_of(&pdf, opts.page)?;
    let mut warnings = Vec::new();
    ctl.progress(0.05);
    // A one-page PDF that is nothing but one picture opens as that picture.
    if count == 1
        && let Some(document) = image_only(name, page)?
    {
        return Ok(ImportResult { document, warnings, source_read_only: false, preview_only: false });
    }
    let info = page_info(page);
    let bx = match opts.crop {
        CropTo::BoundingBox => marks(page, None).bounds_within(info.crop),
        c => info.box_for(c),
    };
    ctl.check().map_err(|_| IoError::Cancelled)?;
    ctl.progress(0.15);
    let ppi = if opts.resolution.is_finite() { opts.resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION) } else { DEFAULT_RESOLUTION };
    let (mut w, mut h) = match opts.pixel_size {
        Some((w, h)) => (w.max(1), h.max(1)),
        None => pixel_size(info.displayed_size(bx), ppi),
    };
    let budget = (MAX_PAGE_PIXELS as f64 / (f64::from(w) * f64::from(h))).sqrt();
    let fit = (f64::from(MAX_SIDE) / f64::from(w)).min(f64::from(MAX_SIDE) / f64::from(h)).min(budget).min(1.0);
    if fit < 1.0 {
        warnings.push(format!("PDF: the {w} × {h} px page was scaled down to fit {MAX_SIDE} px and {} MP", MAX_PAGE_PIXELS / 1_000_000));
        w = ((f64::from(w) * fit).floor() as u32).max(1);
        h = ((f64::from(h) * fit).floor() as u32).max(1);
    }
    let rgba = render_page(page, bx, info.rotation, (w, h), opts.anti_alias);
    ctl.check().map_err(|_| IoError::Cancelled)?;
    ctl.progress(0.8);
    let mode = match opts.mode {
        m @ (ColorMode::Grayscale | ColorMode::Cmyk | ColorMode::Lab) => m,
        _ => ColorMode::Rgb,
    };
    let depth = if opts.depth == SampleType::U16 { SampleType::U16 } else { SampleType::U8 };
    let mut doc = Document::new(page_name(name, opts.name.as_deref(), opts.page, count), Size::new(w, h), mode, depth);
    doc.resolution_dpi = ppi;
    let fmt = doc.pixel_format();
    let px = pixels(&rgba, fmt.mode)?;
    let mut surface = Surface::new(fmt);
    surface.write_region(doc.bounds(), &px);
    surface.prune();
    doc.layers.push(Layer::new("Layer 1", LayerContent::Raster(surface)));
    Ok(ImportResult { document: doc, warnings, source_read_only: true, preview_only: false })
}

/// The document name Photoshop gives page `page` of a `count`-page file: the chosen name or the
/// file's stem, with `-N.pdf` appended when the file has several pages.
pub fn page_name(file: &str, chosen: Option<&str>, page: usize, count: usize) -> String {
    let stem = match chosen.map(str::trim).filter(|s| !s.is_empty()) {
        // The dialog's Name field starts as the file name, `.pdf` included.
        Some(s) => s.strip_suffix(".pdf").or_else(|| s.strip_suffix(".PDF")).unwrap_or(s).to_string(),
        None => stem_of(file),
    };
    if count > 1 { format!("{stem}-{page}.pdf") } else { stem }
}

/// The document name Photoshop gives image `index` (from 1) of a file opened in Images mode:
/// `stem-N`, no extension.
pub fn image_name(file: &str, chosen: Option<&str>, index: usize) -> String {
    let stem = match chosen.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.strip_suffix(".pdf").or_else(|| s.strip_suffix(".PDF")).unwrap_or(s).to_string(),
        None => stem_of(file),
    };
    format!("{stem}-{index}")
}

fn stem_of(file: &str) -> String {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    name.rsplit_once('.').map_or(name.to_string(), |(s, _)| s.to_string())
}

/// A raster image drawn by a page, as the dialog's Images mode lists them: numbered through
/// the file from 1, with the pixel size the file declares. Image masks don't count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfImage {
    pub index: usize,
    pub page: usize,
    pub width: u32,
    pub height: u32,
}

/// Every raster image the file draws, page by page (Photoshop's Images mode).
pub fn images(bytes: &[u8], password: Option<&str>) -> Result<Vec<PdfImage>, IoError> {
    let pdf = load(bytes, password)?;
    let mut out = Vec::new();
    for (p, page) in pdf.pages().iter().enumerate() {
        let m = marks(page, None);
        for placed in m.images {
            out.push(PdfImage { index: out.len() + 1, page: p + 1, width: placed.size.0, height: placed.size.1 });
        }
    }
    Ok(out)
}

/// Image `index` (from 1, counted through the file) as the document Photoshop opens for it in
/// Images mode: the image's own pixels on a `Background` layer, named `stem-N`.
pub fn import_image(name: &str, bytes: &[u8], index: usize, chosen: Option<&str>, password: Option<&str>, ctl: &Interrupt) -> Result<ImportResult, IoError> {
    ctl.check().map_err(|_| IoError::Cancelled)?;
    let pdf = load(bytes, password)?;
    let mut seen = 0usize;
    for page in pdf.pages().iter() {
        // Count this page's images first (no decoding), then decode the one asked for.
        let n = marks(page, None).images.len();
        if index > seen && index <= seen + n {
            ctl.progress(0.3);
            let placed = marks(page, Some(index - seen - 1)).images.into_iter().nth(index - seen - 1);
            let doc = placed
                .and_then(|p| image_document(&image_name(name, chosen, index), &p))
                .ok_or_else(|| IoError::Pdf(format!("image {index} couldn't be decoded")))?;
            return Ok(ImportResult { document: doc, warnings: Vec::new(), source_read_only: false, preview_only: false });
        }
        seen = seen.saturating_add(n);
        ctl.check().map_err(|_| IoError::Cancelled)?;
    }
    Err(IoError::Pdf(format!("image {index} doesn't exist: the PDF has {seen} image(s)")))
}

/// Image `index` (from 1) reduced to fit `max_side`, for the dialog's Images mode.
pub fn image_thumbnail(bytes: &[u8], index: usize, max_side: u32, password: Option<&str>) -> Result<Thumbnail, IoError> {
    let r = import_image("thumb.pdf", bytes, index, None, password, &Interrupt::NONE)?;
    let doc = r.document;
    let (w, h) = (doc.size.width, doc.size.height);
    let k = (max_side.clamp(1, 2048) as f32 / w.max(h).max(1) as f32).min(1.0);
    let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let Some(surface) = doc.layers.first().and_then(|l| l.surface()) else { return Err(IoError::Pdf("no image".to_string())) };
    let n = surface.format().channels();
    let gray = doc.mode == ColorMode::Grayscale;
    let mut rgba = Vec::with_capacity((tw as usize).saturating_mul(th as usize).saturating_mul(4));
    // Nearest-neighbour sampling is plenty for a thumbnail.
    for y in 0..th {
        let sy = ((y as f32 + 0.5) / k).floor().clamp(0.0, (h - 1) as f32) as i32;
        for x in 0..tw {
            let sx = ((x as f32 + 0.5) / k).floor().clamp(0.0, (w - 1) as f32) as i32;
            let px = surface.pixel(sx, sy);
            let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            let (r, g, b) = if gray { (to8(px[0]), to8(px[0]), to8(px[0])) } else { (to8(px[0]), to8(px[1]), to8(px[2])) };
            rgba.extend_from_slice(&[r, g, b, to8(px.get(n - 1).copied().unwrap_or(1.0))]);
        }
    }
    Ok(Thumbnail { width: tw, height: th, rgba })
}

// ------------------------------------------------------------------------------------- reading

fn load(bytes: &[u8], password: Option<&str>) -> Result<Pdf, IoError> {
    let data = Arc::new(bytes.to_vec());
    let r = match password {
        Some(p) => Pdf::new_with_password(data, p),
        None => Pdf::new(data),
    };
    r.map_err(|e| match e {
        LoadPdfError::Decryption(d) => {
            let what = format!("{d:?}");
            if password.is_some() && what.contains("Password") {
                IoError::Pdf(WRONG_PASSWORD.to_string())
            } else if what.contains("Password") {
                IoError::Pdf(PASSWORD_REQUIRED.to_string())
            } else {
                IoError::Pdf(format!("the file's encryption isn't supported ({what})"))
            }
        }
        LoadPdfError::Invalid => IoError::Pdf("not a PDF that can be read (damaged, truncated or not a PDF at all)".to_string()),
    })
}

fn page_of<'a>(pdf: &'a Pdf, page: usize) -> Result<&'a Page<'a>, IoError> {
    let n = pdf.pages().len();
    if n == 0 {
        return Err(IoError::Pdf("the PDF has no pages".to_string()));
    }
    pdf.pages().get(page.wrapping_sub(1)).ok_or_else(|| IoError::Pdf(format!("page {page} doesn't exist: the PDF has {n} page(s)")))
}

/// `[x0, y0, x1, y1]` with the corners ordered.
fn norm(r: hayro::hayro_syntax::object::Rect) -> [f32; 4] {
    [r.x0.min(r.x1) as f32, r.y0.min(r.y1) as f32, r.x0.max(r.x1) as f32, r.y0.max(r.y1) as f32]
}

fn inter(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (x0, y0) = (a[0].max(b[0]), a[1].max(b[1]));
    [x0, y0, a[2].min(b[2]).max(x0), a[3].min(b[3]).max(y0)]
}

fn has_area(b: [f32; 4]) -> bool {
    b[2] > b[0] && b[3] > b[1]
}

fn to_pt(b: [f32; 4]) -> PtRect {
    PtRect::new(f64::from(b[0]), f64::from(b[1]), f64::from(b[2]), f64::from(b[3]))
}

fn page_info(p: &Page) -> PdfPage {
    // A page without a usable media box is treated as A4, as hayro renders it.
    let mut media = norm(p.media_box());
    if !has_area(media) {
        media = [0.0, 0.0, 595.276, 841.89];
    }
    let mut crop = inter(norm(p.crop_box()), media);
    if !has_area(crop) {
        crop = media;
    }
    let get = |key: &[u8]| p.raw().get::<hayro::hayro_syntax::object::Rect>(key).map(norm).map(|b| inter(b, media)).filter(|b| has_area(*b)).unwrap_or(crop);
    let rotation = match p.rotation() {
        Rotation::None => 0,
        Rotation::Horizontal => 90,
        Rotation::Flipped => 180,
        Rotation::FlippedHorizontal => 270,
    };
    PdfPage { media, crop, bleed: get(b"BleedBox"), trim: get(b"TrimBox"), art: get(b"ArtBox"), rotation }
}

fn settings() -> InterpreterSettings {
    InterpreterSettings::default()
}

// ---------------------------------------------------------------------------------- rendering

/// PDF points of box `bx` → the unrotated page with y down, then turned by `rotation`, at one
/// pixel per point (the page's own `initial_transform`, for any box).
fn oriented(bx: [f32; 4], rotation: u16) -> Affine {
    let (x0, y1) = (f64::from(bx[0]), f64::from(bx[3]));
    let (w, h) = (f64::from(bx[2] - bx[0]), f64::from(bx[3] - bx[1]));
    let upright = Affine::new([1.0, 0.0, 0.0, -1.0, -x0, y1]);
    let turn = match rotation {
        90 => Affine::new([0.0, 1.0, -1.0, 0.0, h, 0.0]),
        180 => Affine::new([-1.0, 0.0, 0.0, -1.0, w, h]),
        270 => Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, w]),
        _ => Affine::IDENTITY,
    };
    turn * upright
}

/// Box `bx` of `page` rendered into `w × h` pixels, straight-alpha RGBA, transparent where the
/// page paints nothing.
fn render_page(page: &Page, bx: [f32; 4], rotation: u16, (w, h): (u32, u32), anti_alias: bool) -> Vec<u8> {
    let (w, h) = (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE));
    let (bw, bh) = (f64::from(bx[2] - bx[0]).max(1.0e-6), f64::from(bx[3] - bx[1]).max(1.0e-6));
    let (dw, dh) = if rotation == 90 || rotation == 270 { (bh, bw) } else { (bw, bh) };
    let transform = Affine::scale_non_uniform(f64::from(w) / dw, f64::from(h) / dh) * oriented(bx, rotation);
    // The page renders on this thread; PhotoCraft's own parallelism runs around it.
    let render_settings = vello_cpu::RenderSettings { num_threads: 0, ..Default::default() };
    let mut ctx = RenderContext::new_with(w as u16, h as u16, render_settings);
    render_into(page, &RenderCache::new(), &settings(), &RenderSettings::default(), &mut ctx, transform);
    ctx.flush();
    let mut pixmap = Pixmap::new(w as u16, h as u16);
    ctx.render_with(
        &mut pixmap,
        &mut vello_cpu::Resources::default(),
        vello_cpu::RasterizerSettings { target_init: vello_cpu::TargetInit::Clear(TRANSPARENT), ..Default::default() },
    );
    let mut rgba = pixmap.take_rgba8(ImageAlphaType::Alpha);
    if !anti_alias {
        // Anti-aliased off: a pixel is in or out. The renderer resets its own aliasing threshold
        // while hayro drives it, so the coverage is settled afterwards: half or more is in.
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = if px[3] >= 128 { 255 } else { 0 };
        }
    }
    rgba
}

/// Straight-alpha sRGB bytes → the document's interleaved normalised channels: sRGB as is, and
/// the other modes through the working profiles Photoshop assigns (Dot Gain 20 %, the coated
/// CMYK profile, Lab D50), relative colorimetric with black point compensation.
fn pixels(rgba: &[u8], mode: ColorMode) -> Result<Vec<f32>, IoError> {
    let src: Vec<f32> = rgba.iter().map(|&v| f32::from(v) / 255.0).collect();
    let dst = match mode {
        ColorMode::Grayscale => Builtin::DotGain20Compat,
        ColorMode::Cmyk => Builtin::CoatedCmyk,
        ColorMode::Lab => Builtin::LabD50,
        _ => return Ok(src),
    };
    let n = mode.color_channels();
    let t = Transform::new(Builtin::Srgb.profile(), dst.profile(), Intent::RelativeColorimetric, true)
        .map_err(|e| IoError::Pdf(format!("colour conversion: {e}")))?;
    let mut out = vec![0.0f32; src.len() / 4 * (n + 1)];
    t.convert_f32(&src, 4, &mut out, n + 1, true);
    Ok(out)
}

// ------------------------------------------------------------------------------------- marks

/// What a page draws, as a [`Device`]: the extent of every mark (clipped), counts per kind, and
/// the raster images with their placement (decoded when asked, for the image-only shortcut).
struct Marks {
    clips: Vec<PtRect>,
    bounds: Option<PtRect>,
    paths: usize,
    glyphs: usize,
    stencils: usize,
    /// Which raster image (by order of drawing) to decode; the others are only placed.
    decode: Option<usize>,
    images: Vec<Placed>,
}

struct Placed {
    transform: Affine,
    /// The pixel size the file declares.
    size: (u32, u32),
    decoded: Option<(ImageData, Option<LumaData>)>,
}

impl Marks {
    fn mark(&mut self, r: PtRect) {
        let r = match self.clips.last() {
            Some(c) => r.intersect(*c),
            None => r,
        };
        if !(r.width() > 0.0 && r.height() > 0.0) || !r.x0.is_finite() || !r.y0.is_finite() || !r.x1.is_finite() || !r.y1.is_finite() {
            return;
        }
        self.bounds = Some(self.bounds.map_or(r, |b| b.union(r)));
    }

    /// The marks' box cut to `within`, or `within` itself for a page that draws nothing.
    fn bounds_within(&self, within: [f32; 4]) -> [f32; 4] {
        match self.bounds {
            Some(b) => {
                let r = inter([b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32], within);
                if has_area(r) { r } else { within }
            }
            None => within,
        }
    }
}

/// Interprets `page` through a [`Marks`] device, in PDF points (y up); `decode` picks the raster
/// image (by drawing order) whose pixels are wanted.
fn marks(page: &Page, decode: Option<usize>) -> Marks {
    let mut device = Marks { clips: Vec::new(), bounds: None, paths: 0, glyphs: 0, stencils: 0, decode, images: Vec::new() };
    let cache = InterpreterCache::new();
    let media = to_pt(norm(page.media_box()));
    let mut context = Context::new(Affine::IDENTITY, media, &cache, page.xref(), settings());
    interpret_page(page, &mut context, &mut device);
    device
}

impl<'a> Device<'a> for Marks {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, draw_mode: &DrawMode) {
        let mut r = path.bounding_box();
        if let DrawMode::Stroke(s) | DrawMode::FillAndStroke(_, s) = draw_mode {
            let grow = (f64::from(s.line_width) / 2.0).max(4.0) + 2.0;
            r = r.inflate(grow, grow);
        }
        self.mark(props.transform.transform_rect_bbox(r));
        self.paths = self.paths.saturating_add(1);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        let r = clip_path.path.bounding_box();
        let r = match self.clips.last() {
            Some(c) => r.intersect(*c),
            None => r,
        };
        self.clips.push(r);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, glyph_run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        let b = PtRect::new(GLYPH_BOX[0], GLYPH_BOX[1], GLYPH_BOX[2], GLYPH_BOX[3]);
        for g in glyph_run.glyphs() {
            self.mark((props.transform * g.transform()).transform_rect_bbox(b));
            self.glyphs = self.glyphs.saturating_add(1);
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        // The transform places the image's own pixel grid (top row first).
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        self.mark(props.transform.transform_rect_bbox(PtRect::new(0.0, 0.0, w, h)));
        match image {
            Image::Stencil(_) => self.stencils = self.stencils.saturating_add(1),
            Image::Raster(r) => {
                let mut decoded = None;
                if self.decode == Some(self.images.len()) {
                    r.with_rgba(|data, alpha| decoded = Some((data, alpha)), None);
                }
                self.images.push(Placed { transform: props.transform, size: (r.width(), r.height()), decoded });
            }
        }
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn pop_transparency_group(&mut self) {}
}

/// The image of a page that draws exactly one raster image and nothing else, as the document
/// Photoshop opens for it (see [`image_document`]). `None` for any other page.
fn image_only(name: &str, page: &Page) -> Result<Option<Document>, IoError> {
    let m = marks(page, Some(0));
    if m.paths != 0 || m.glyphs != 0 || m.stencils != 0 || m.images.len() != 1 {
        return Ok(None);
    }
    Ok(m.images.first().and_then(|p| image_document(name, p)))
}

/// A placed, decoded image as Photoshop opens it: its own pixels on a locked `Background`
/// layer, Grayscale or RGB as the image is, 8 bits, at the resolution its placed width implies
/// (rotation and uneven scaling in the matrix are ignored, as Photoshop ignores them). `None`
/// when it wasn't decoded or is absurd.
fn image_document(name: &str, placed: &Placed) -> Option<Document> {
    let (data, alpha) = placed.decoded.as_ref()?;
    let (w, h) = (data.width(), data.height());
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PAGE_PIXELS {
        return None;
    }
    // The placed width in points: the declared pixel width along the matrix's x axis.
    let c = placed.transform.as_coeffs();
    let placed_pt = c[0].hypot(c[1]) * f64::from(placed.size.0);
    let ppi = if placed_pt > 1.0e-6 { (f64::from(w) * 72.0 / placed_pt) as f32 } else { 72.0 };
    let (mode, n, color): (ColorMode, usize, &[u8]) = match data {
        ImageData::Luma(l) => (ColorMode::Grayscale, 1, &l.data),
        ImageData::Rgb(r) => (ColorMode::Rgb, 3, &r.data),
    };
    let count = (w as usize).saturating_mul(h as usize);
    if color.len() < count.saturating_mul(n) {
        return None;
    }
    let alpha = alpha.as_ref().filter(|a| a.width == w && a.height == h && a.data.len() >= count);
    let mut doc = Document::new(name, Size::new(w, h), mode, SampleType::U8);
    doc.resolution_dpi = ppi.clamp(MIN_RESOLUTION, MAX_RESOLUTION);
    let fmt = doc.pixel_format();
    let mut px = Vec::with_capacity(count.saturating_mul(n + 1));
    for (i, p) in color.chunks_exact(n).take(count).enumerate() {
        px.extend(p.iter().map(|&v| f32::from(v) / 255.0));
        px.push(alpha.and_then(|a| a.data.get(i)).map_or(1.0, |&a| f32::from(a) / 255.0));
    }
    let mut layer = Layer::raster("Background", fmt);
    layer.locks.transparency = true;
    layer.locks.position = true;
    if let Some(s) = layer.surface_mut() {
        s.write_region(doc.bounds(), &px);
    }
    doc.layers.push(layer);
    Some(doc)
}

#[cfg(test)]
mod tests;
