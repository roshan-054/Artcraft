use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use photocraft_engine::Session;
use serde_json::{Value, json};

use super::*;
use crate::{PhotocraftApp, Services};

/// A hand-written PDF: `pages` are (width, height, content stream).
fn pdf(pages: &[(f32, f32, &str)]) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut objs: Vec<Vec<u8>> =
        vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).into_bytes()];
    for (i, (w, h, content)) in pages.iter().enumerate() {
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes());
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R >>", 3 + 2 * i).into_bytes());
    }
    write(objs)
}

/// A one-page PDF drawing one uncompressed RGB image of `w × h` pixels, `placed` wide in points.
fn image_pdf(w: u32, h: u32, placed: f32) -> Vec<u8> {
    let px: Vec<u8> = (0..w * h * 3).map(|k| (k % 251) as u8).collect();
    let mut img =
        format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n", px.len())
            .into_bytes();
    img.extend_from_slice(&px);
    img.extend_from_slice(b"\nendstream");
    let content = format!("q {placed} 0 0 {} 50 50 cm /Im0 Do Q", placed * h as f32 / w as f32);
    let objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        img,
    ];
    write(objs)
}

fn write(objs: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn app() -> PhotocraftApp {
    PhotocraftApp::new(
        Session::new(),
        Services {
            import: Some(Box::new(|name: &str, bytes: &[u8], opts: &photocraft_io::ImportOptions| {
                photocraft_io::import_with_options(name, bytes, opts).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
            })),
            ..Default::default()
        },
    )
}

fn three_pages() -> Vec<u8> {
    pdf(&[(612.0, 792.0, "1 0 0 rg 72 72 468 648 re f"), (595.0, 420.0, "0 1 0 rg 30 30 535 360 re f"), (300.0, 300.0, "0 0 1 rg 20 20 260 260 re f")])
}

fn dialog(app: &PhotocraftApp) -> Option<&crate::state::Dialog> {
    app.ui.dialogs.iter().find(|d| owns(&d.fields))
}

fn set(app: &mut PhotocraftApp, key: &str, v: Value) {
    let id = dialog(app).expect("the Import PDF dialog").id;
    app.ui.dialog_mut(id).expect("dialog").fields.insert(key.into(), v);
}

#[test]
fn opening_a_pdf_shows_the_dialog_with_photoshop_defaults() {
    let mut app = app();
    let warnings = app.open_file("/tmp/pc-test/multi.pdf", &three_pages()).expect("open");
    assert!(warnings.is_empty());
    assert!(app.session.documents().is_empty(), "nothing opens before OK");
    let d = dialog(&app).expect("the Import PDF dialog");
    let f = &d.fields;
    assert_eq!(f.get("name"), Some(&json!("multi.pdf")), "the Name field starts as the file name");
    assert_eq!(f.get("select"), Some(&json!("pages")));
    assert_eq!(f.get("pages"), Some(&json!([1])));
    assert_eq!(f.get("crop"), Some(&json!("boundingBox")));
    assert_eq!(f.get("antiAlias"), Some(&json!(true)));
    assert_eq!(f.get("resolution"), Some(&json!(300.0)));
    assert_eq!(f.get("mode"), Some(&json!("rgb")));
    assert_eq!(f.get("depth"), Some(&json!(8)));
    assert_eq!(f.get("constrain"), Some(&json!(false)));
    assert_eq!(f.get("suppressWarnings"), Some(&json!(false)));
    assert_eq!(f.get("reverseOrder"), Some(&json!(true)));
    assert_eq!(f.get("thumbnailSize"), Some(&json!("small")));
    let st = app.pdf_import.as_ref().expect("the file is held");
    assert_eq!(st.info.as_ref().map(|i| i.pages.len()), Some(3));
    assert_eq!(st.path.as_deref(), Some("/tmp/pc-test/multi.pdf"));
}

#[test]
fn ok_opens_the_selected_pages_in_reverse_order_as_unsaved_documents() {
    let mut app = app();
    app.open_file("/tmp/pc-test/multi.pdf", &three_pages()).expect("open");
    let id = dialog(&app).unwrap().id;
    set(&mut app, "pages", json!([1, 2, 3]));
    set(&mut app, "resolution", json!(72.0));
    set(&mut app, "crop", json!("mediaBox"));
    let r = crate::dialogs::confirm(&mut app, id).expect("OK");
    assert_eq!(r.get("opened"), Some(&json!(3)));
    assert!(dialog(&app).is_none() && app.pdf_import.is_none());
    let names: Vec<&str> = app.session.documents().iter().map(|d| d.doc.name.as_str()).collect();
    assert_eq!(names, ["multi-3.pdf", "multi-2.pdf", "multi-1.pdf"], "last page first, so page 1 ends up on top");
    let active = app.session.active().expect("a document");
    assert_eq!(active.doc.name, "multi-1.pdf");
    assert_eq!((active.doc.size.width, active.doc.size.height, active.doc.resolution_dpi), (612, 792, 72.0));
    assert!(active.source_read_only && active.path.is_none(), "Save must not write into the PDF");
    assert_eq!(app.session.documents()[0].doc.size.width, 300);
    assert!(app.ui.recent_files.iter().any(|p| p.ends_with("multi.pdf")), "the PDF goes to Open Recent");
}

#[test]
fn the_name_field_and_the_page_options_reach_the_documents() {
    let mut app = app();
    app.open_bytes("multi.pdf", &three_pages()).expect("open");
    let id = dialog(&app).unwrap().id;
    set(&mut app, "name", json!("Foo"));
    set(&mut app, "pages", json!([2]));
    set(&mut app, "resolution", json!(72.0));
    set(&mut app, "mode", json!("gray"));
    set(&mut app, "depth", json!(16));
    set(&mut app, "crop", json!("boundingBox"));
    set(&mut app, "reverseOrder", json!(false));
    crate::dialogs::confirm(&mut app, id).expect("OK");
    let doc = &app.session.active().unwrap().doc;
    assert_eq!(doc.name, "Foo-2.pdf");
    assert_eq!((doc.size.width, doc.size.height), (535, 360), "the bounding box of the green rectangle");
    assert_eq!((doc.mode, doc.depth), (photocraft_color::ColorMode::Grayscale, photocraft_color::SampleType::U16));
    // The options the fields describe.
    let o = options(&dialog_fields_after(&app), 2, false, None);
    assert_eq!((o.page, o.crop, o.resolution, o.anti_alias, o.image), (2, CropTo::BoundingBox, 72.0, true, None));
}

/// The fields a closed dialog had, rebuilt from defaults plus the test's edits (the dialog is
/// gone after OK, so this stands in for them).
fn dialog_fields_after(app: &PhotocraftApp) -> Map<String, Value> {
    let mut f = Map::new();
    defaults(&mut f, app);
    f.insert("resolution".into(), json!(72.0));
    f.insert("mode".into(), json!("gray"));
    f
}

#[test]
fn cancel_opens_nothing_and_lets_the_file_go() {
    let mut app = app();
    app.open_bytes("multi.pdf", &three_pages()).expect("open");
    let id = dialog(&app).unwrap().id;
    crate::dialogs::cancel(&mut app, id).expect("cancel");
    assert!(app.session.documents().is_empty());
    assert!(app.pdf_import.is_none() && dialog(&app).is_none());
}

#[test]
fn ok_with_nothing_selected_keeps_the_dialog_up() {
    let mut app = app();
    app.open_bytes("multi.pdf", &three_pages()).expect("open");
    let id = dialog(&app).unwrap().id;
    set(&mut app, "pages", json!([]));
    let r = crate::dialogs::confirm(&mut app, id).expect("OK");
    assert_eq!(r.get("pending"), Some(&json!(true)));
    assert!(app.session.documents().is_empty());
    let d = dialog(&app).expect("still up");
    assert!(d.fields.get("__error").is_some());
    assert!(app.pdf_import.is_some());
}

#[test]
fn a_one_image_pdf_opens_like_an_image_without_asking() {
    let mut app = app();
    assert!(!wants_dialog(&image_pdf(100, 50, 200.0)));
    app.open_file("/tmp/pc-test/scan.pdf", &image_pdf(100, 50, 200.0)).expect("open");
    assert!(dialog(&app).is_none());
    let st = app.session.active().expect("the image document");
    assert_eq!((st.doc.name.as_str(), st.doc.size.width, st.doc.size.height, st.doc.resolution_dpi), ("scan.pdf", 100, 50, 36.0));
    assert_eq!(st.doc.layers[0].name, "Background");
    assert!(!st.source_read_only && st.path.as_deref() == Some("/tmp/pc-test/scan.pdf"), "like any image file");
}

#[test]
fn images_mode_opens_the_embedded_images() {
    // Two pages: the second draws an image beside a rectangle, so the file isn't image-only.
    let px: Vec<u8> = (0..8 * 4 * 3).map(|k| (k % 251) as u8).collect();
    let mut img = format!("<< /Type /XObject /Subtype /Image /Width 8 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n", px.len())
        .into_bytes();
    img.extend_from_slice(&px);
    img.extend_from_slice(b"\nendstream");
    let c1 = "1 0 0 rg 0 0 50 50 re f";
    let c2 = "q 80 0 0 40 100 100 cm /Im0 Do Q 0 0 1 rg 0 0 10 10 re f";
    let objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{c1}\nendstream", c1.len()).into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Resources << /XObject << /Im0 7 0 R >> >> /Contents 6 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{c2}\nendstream", c2.len()).into_bytes(),
        img,
    ];
    let bytes = write(objs);
    let mut app = app();
    app.open_bytes("pair.pdf", &bytes).expect("open");
    let id = dialog(&app).unwrap().id;
    set(&mut app, "select", json!("images"));
    set(&mut app, "images", json!([1]));
    crate::dialogs::confirm(&mut app, id).expect("OK");
    let doc = &app.session.active().unwrap().doc;
    assert_eq!((doc.name.as_str(), doc.size.width, doc.size.height), ("pair-1", 8, 4));
    assert_eq!(doc.layers[0].name, "Background");
}

#[test]
fn size_label_prints_three_significant_digits_like_photoshop() {
    assert_eq!(size_label(2550, 2856, "rgb", 8), "20.8M");
    assert_eq!(size_label(1000, 1120, "rgb", 8), "3.20M");
    assert_eq!(size_label(1000, 1120, "cmyk", 16), "8.54M");
    assert_eq!(size_label(100, 100, "gray", 8), "9.77K");
    assert_eq!(size_label(7000, 7000, "rgb", 8), "140M");
}

#[test]
fn the_dialog_draws_its_controls() {
    let mut app = app();
    app.open_bytes("multi.pdf", &three_pages()).expect("open");
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            crate::dialogs::show(app, &ctx);
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ALL[0]);
    h.run_steps(6);
    for label in [
        "Import PDF",
        "Select:",
        "Pages",
        "Images",
        "Page Options",
        "Anti-aliased",
        "Constrain Proportions",
        "Suppress Warnings",
        "Open In Reverse Order",
        "Thumbnail Size:",
        "1 of 3 page(s) selected",
        "Page 1",
    ] {
        assert!(h.query_by_label(label).is_some(), "missing {label}");
    }
    // The thumbnails were rendered.
    let st = h.state().pdf_import.as_ref().expect("held");
    assert!(st.thumbs.iter().all(Option::is_some), "every page has its thumbnail");
}
