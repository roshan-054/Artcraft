//! The command registry. Ids follow Photoshop's menu structure (see the parity checklist).

use photocraft_color::{BlendMode, Color, ColorMode, SampleType};
use photocraft_doc::{Adjustment, Document, Fill, Layer, LayerContent, LayerId, LayerMask, Size};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Map, Value, json};

use crate::{EngineError, Result, Session, inspect, pixels};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

/// Metadata + implementation for one command.
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Layer", "New"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    /// Default shortcut in egui-style notation (`Cmd+Shift+N`), mapped per platform by the UI.
    pub shortcut: Option<&'static str>,
    /// Human/agent-readable parameter description (JSON-ish).
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    /// Record in the session journal (false for queries).
    pub journal: bool,
}

pub(crate) fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}
fn has_layer(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    d.active_layer.filter(|id| d.doc.layer(*id).is_some()).map(|_| ()).ok_or_else(|| "no active layer".into())
}
fn has_pixel_layer(s: &Session) -> std::result::Result<(), String> {
    let l = crate::active_layer_of(s)?;
    if matches!(l.content, LayerContent::Raster(_)) {
        Ok(())
    } else {
        Err(format!("active layer is {} {} layer, not a pixel layer", l.content.article(), l.content.kind_name()))
    }
}
/// A pixel layer, or a targeted alpha channel / Quick Mask (adjustments and fills apply to it).
fn has_pixel_or_channel(s: &Session) -> std::result::Result<(), String> {
    if crate::channel_cmds::edits_channel(s) {
        return Ok(());
    }
    has_pixel_layer(s)
}
/// A layer that can be painted: pixels, or any layer with a mask (paint with `"target":"mask"`).
pub(crate) fn has_paintable(s: &Session) -> std::result::Result<(), String> {
    if crate::channel_cmds::edits_channel(s) {
        return Ok(());
    }
    let l = crate::active_layer_of(s)?;
    if matches!(l.content, LayerContent::Raster(_)) || l.mask.is_some() {
        Ok(())
    } else {
        Err(format!("active layer is {} {} layer without a mask", l.content.article(), l.content.kind_name()))
    }
}

/// Surface a paint command writes to: the layer's pixels, or its mask with `"target":"mask"`.
pub(crate) fn paint_surface<'a>(doc: &'a mut Document, id: LayerId, p: &Value) -> Result<&'a mut photocraft_raster::Surface> {
    if is_mask_target(p) {
        check_all_unlocked(doc, id)?;
    } else {
        check_pixels_unlocked(doc, id)?;
    }
    let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
    if is_mask_target(p) {
        l.mask.as_mut().map(|m| &mut m.surface).ok_or_else(|| EngineError::Other("layer has no mask".into()))
    } else {
        l.surface_mut().ok_or_else(|| EngineError::Other("not a pixel layer".into()))
    }
}

fn check_all_unlocked(doc: &Document, id: LayerId) -> Result<()> {
    let locks = doc.effective_locks(id);
    let l = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    if locks.all {
        return Err(EngineError::Other(format!("Could not complete your request because the layer \"{}\" is locked", l.name)));
    }
    Ok(())
}

/// Refuse a layer whose pixels are locked (directly or through a group), with Photoshop's message.
pub(crate) fn check_pixels_unlocked(doc: &Document, id: LayerId) -> Result<()> {
    let locks = doc.effective_locks(id);
    let l = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    if locks.pixels || locks.all {
        return Err(EngineError::Other(format!("Could not complete your request because the layer \"{}\" is locked", l.name)));
    }
    Ok(())
}

pub(crate) fn is_mask_target(p: &Value) -> bool {
    p.get("target").and_then(Value::as_str) == Some("mask")
}

fn can_undo(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|d| d.history.can_undo()).map(|_| ()).ok_or_else(|| "nothing to undo".into())
}
fn can_redo(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|d| d.history.can_redo()).map(|_| ()).ok_or_else(|| "nothing to redo".into())
}
fn has_selection(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|d| d.doc.selection.is_some()).map(|_| ()).ok_or_else(|| "no selection".into())
}

/// Image › Image Rotation by a right angle or a flip (pixels, vectors, guides, … all move).
fn turn(s: &mut Session, label: &str, t: crate::canvas_geom::Turn) -> Result<Value> {
    if let Some(d) = s.active() {
        let a = t.affine(f64::from(d.doc.size.width), f64::from(d.doc.size.height));
        s.set_sampler_map(a);
    }
    s.edit(label, |doc, _| {
        crate::canvas_geom::turn_canvas(doc, t);
        Ok(())
    })?;
    Ok(Value::Null)
}

macro_rules! cmd {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: true }
    };
}

mod adjustments;
mod edit_menu;
mod file_menu;
mod image_menu;
mod layer_menu;
mod paint;
mod queries;
mod select_menu;

pub fn command_specs() -> &'static [CommandSpec] {
    &registry().specs
}

/// One `command.list` / `engine.commands` row for `c`, given why it can't run now (`None` = it can).
/// `disabledReason` is that message; `needsDocument` is true when the reason is that no document is
/// open (#3664), so an agent can tell "open or create a document first" from a precondition that a
/// document alone won't meet (a selection, an empty clipboard).
pub fn list_entry(s: &Session, c: &CommandSpec, reason: Option<String>) -> Value {
    let needs_document =
        s.active().is_none() && reason.as_deref().is_some_and(|r| ["no document", "no active document", "no open document"].iter().any(|p| r.starts_with(p)));
    let mut entry = json!({
        "id": c.id, "label": c.label, "menu": c.menu, "shortcut": c.shortcut, "params": c.params,
        "enabled": reason.is_none(), "disabledReason": reason, "needsDocument": needs_document,
    });
    if let (Some(k), Some(o)) = (search_keywords(c.id), entry.as_object_mut()) {
        o.insert("keywords".into(), json!(k));
    }
    entry
}

/// Other names a command is looked for by, beyond its id and label: the Photoshop tools and
/// options it stands for (#5060). `select.rect` is also the Elliptical and Single Row/Column
/// Marquee, which an agent searches for as "ellipse" or "marquee".
pub(crate) const SEARCH_KEYWORDS: &[(&str, &str)] =
    &[("select.rect", "Rectangular Marquee, Elliptical Marquee (\"ellipse\":true), ellipse, oval, circle, Single Row/Column Marquee (\"line\")")];

/// [`SEARCH_KEYWORDS`] of command `id`, if it has any.
pub fn search_keywords(id: &str) -> Option<&'static str> {
    SEARCH_KEYWORDS.iter().find(|(k, _)| *k == id).map(|(_, words)| *words)
}

/// Does a [`list_entry`] row match a command-list filter (`commands --filter`, `engine.commands`,
/// MCP `command_list`)? Its id, label or keywords contain `needle`, ignoring case.
pub fn list_entry_matches(entry: &Value, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    ["id", "label", "keywords"].iter().any(|k| entry.get(*k).and_then(Value::as_str).is_some_and(|v| v.to_lowercase().contains(&needle)))
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    let registry = registry();
    registry.by_id.get(id).and_then(|&index| registry.specs.get(index))
}

struct Registry {
    specs: Vec<CommandSpec>,
    by_id: std::collections::HashMap<&'static str, usize>,
}

impl Registry {
    fn new(specs: Vec<CommandSpec>) -> Self {
        let mut by_id = std::collections::HashMap::with_capacity(specs.len());
        for (index, spec) in specs.iter().enumerate() {
            // Preserve the linear lookup's first-match behavior if an id is duplicated.
            by_id.entry(spec.id).or_insert(index);
        }
        Self { specs, by_id }
    }
}

fn registry() -> &'static Registry {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Registry::new(build()))
}

// ---------- param helpers ----------

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
/// Integer parameter that also accepts JSON floats (UIs send `12.0`); rounds.
pub(crate) fn int(p: &Value, key: &str) -> Option<i64> {
    p.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64)))
}

/// [`int`] narrowed to `i32` for geometry: `None` when absent, and a bad-params error when the
/// value would wrap through `as i32` — out-of-range integers truncate by multiples of 2^32,
/// which silently relocates geometry (`dx = 2^32 + 50` moves 50 px, `3e9` goes negative)
/// instead of erroring.
pub(crate) fn int_i32(cmd: &str, p: &Value, key: &str) -> Result<Option<i32>> {
    match int(p, key) {
        None => Ok(None),
        Some(v) => i32::try_from(v).map(Some).map_err(|_| bad(cmd, format!("`{key}` = {v} is outside the 32-bit coordinate range"))),
    }
}

/// An id (layer comp, slice, style…) from a JSON value: a bad-params error unless it is a whole
/// number in `u32` range. `as u32` would wrap `2^32 + 1` to `1` and target a real item.
pub(crate) fn u32_id(cmd: &str, key: &str, v: &Value) -> Result<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(cmd, format!("`{key}` = {v} is not a valid id (0..={})", u32::MAX)))
}

/// [`u32_id`] of `p[key]`: `None` when absent.
pub(crate) fn u32_id_param(cmd: &str, p: &Value, key: &str) -> Result<Option<u32>> {
    p.get(key).map(|v| u32_id(cmd, key, v)).transpose()
}

fn f32_or(p: &Value, key: &str, default: f32) -> f32 {
    p.get(key).and_then(Value::as_f64).map(|v| v as f32).unwrap_or(default)
}
pub(crate) fn color_param(p: &Value, key: &str, default: [f32; 4]) -> [f32; 4] {
    match p.get(key) {
        Some(Value::Array(a)) if a.len() >= 3 => {
            let v: Vec<f32> = a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
            [v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0)]
        }
        Some(Value::String(s)) => parse_hex(s).unwrap_or(default),
        _ => default,
    }
}
pub(crate) fn parse_hex(s: &str) -> Option<[f32; 4]> {
    let s = s.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok().map(|v| v as f32 / 255.0);
    match s.len() {
        6 => Some([b(0)?, b(2)?, b(4)?, 1.0]),
        8 => Some([b(0)?, b(2)?, b(4)?, b(6)?]),
        _ => None,
    }
}
/// Layer › Create / Release Clipping Mask. With a `layer` param, just that layer. Otherwise, as in
/// Photoshop, the selection (#1248): Create clips every selected layer except the lowest, which
/// becomes the base; Release releases every selected layer. Releasing a clipped layer also
/// releases the clipped layers above it in its group, as Adobe documents ("removes the selected
/// layer and any layers above it from the clipping mask"), so none is left clipped to a layer
/// that is no longer clipped. One history step either way.
fn set_clipped(s: &mut Session, p: &Value, clip: bool) -> Result<Value> {
    let label = if clip { "Create Clipping Mask" } else { "Release Clipping Mask" };
    let ids: Vec<LayerId> = if p.get("layer").is_some() {
        vec![layer_param(s, p)?]
    } else {
        let selected = s.active().ok_or(EngineError::NoDocument)?.selected_layers();
        match (clip, selected.as_slice()) {
            (_, []) => vec![layer_param(s, p)?],
            (_, [one]) => vec![*one],
            (true, [_base, rest @ ..]) => rest.to_vec(),
            (false, all) => all.to_vec(),
        }
    };
    let ids = if clip {
        ids
    } else {
        let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
        let mut all = ids.clone();
        for id in ids.iter().filter(|id| doc.layer(**id).is_some_and(|l| l.clipped)) {
            for above in crate::clip_group::clipped_above(doc, *id) {
                if !all.contains(&above) {
                    all.push(above);
                }
            }
        }
        all
    };
    s.edit(label, |doc, _| {
        for id in &ids {
            doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?.clipped = clip;
        }
        Ok(())
    })?;
    Ok(json!({"layers": ids.iter().map(|id| id.0).collect::<Vec<_>>()}))
}

/// The layer a command's `"layer"` param names: an id, or a name (the topmost layer of that
/// name, as Photoshop's "select layer by name" action step finds it, so a loaded action replays
/// in another document; a name that matches no layer exactly matches regardless of case, so
/// `"background"` finds the Background). `None` when the param is absent.
pub(crate) fn explicit_layer(s: &Session, p: &Value) -> Result<Option<LayerId>> {
    match p.get("layer") {
        Some(Value::Number(n)) => {
            n.as_u64().map(|id| Some(LayerId(id))).ok_or_else(|| EngineError::Other(format!("layer id {n} is not a non-negative integer")))
        }
        Some(Value::String(name)) => {
            let d = s.active().ok_or(EngineError::NoDocument)?;
            let layers = d.doc.walk();
            let mut top_down = layers.iter().rev();
            top_down
                .clone()
                .find(|(_, _, l)| l.name == *name)
                .or_else(|| top_down.find(|(_, _, l)| l.name.to_lowercase() == name.to_lowercase()))
                .map(|(_, _, l)| Some(l.id))
                .ok_or_else(|| EngineError::Other(format!("no layer named \"{name}\"; {}", layer_listing(&d.doc))))
        }
        Some(Value::Null) | None => Ok(None),
        Some(other) => Err(EngineError::Other(format!("`layer` must be an id or a name, not {other}"))),
    }
}

/// The layers of `doc` as a caller can name them, for an error about a layer that isn't there:
/// `the document's layers are 2 "Background", 3 "Layer 1"` (top-down, at most 20).
pub(crate) fn layer_listing(doc: &Document) -> String {
    const MAX: usize = 20;
    let layers = doc.walk();
    if layers.is_empty() {
        return "the document has no layers".into();
    }
    // Names are unbounded input: each one is cut at NAME characters so the error stays short.
    const NAME: usize = 64;
    let name = |n: &str| match n.char_indices().nth(NAME) {
        Some((cut, _)) => format!("{}…", &n[..cut]),
        None => n.to_string(),
    };
    let mut list: Vec<String> = layers.iter().rev().take(MAX).map(|(_, _, l)| format!("{} {:?}", l.id.0, name(&l.name))).collect();
    if layers.len() > MAX {
        list.push(format!("… {} more (see `document.inspect`)", layers.len() - MAX));
    }
    format!("the document's layers are {} (a `layer` param takes an id or a name)", list.join(", "))
}

/// The layer a command targets: [`explicit_layer`], else the active layer.
pub(crate) fn layer_param(s: &Session, p: &Value) -> Result<LayerId> {
    match explicit_layer(s, p)? {
        Some(id) => Ok(id),
        None => s.active().and_then(|d| d.active_layer).ok_or(EngineError::Other("no active layer".into())),
    }
}
pub fn blend_from_str(s: &str) -> Option<BlendMode> {
    let norm = |x: &str| x.to_ascii_lowercase().replace([' ', '_', '-', '(', ')'], "");
    let want = norm(s);
    std::iter::once(BlendMode::PassThrough).chain(BlendMode::layer_modes()).find(|m| norm(m.label()) == want || norm(&format!("{m:?}")) == want)
}

/// The active selection as the mask of a layer being created, or None without a selection.
/// Photoshop masks a new adjustment or fill layer with the selection, so the adjustment acts on
/// the selected area only (#1250); the selection itself stays, as Reveal Selection leaves it.
pub(crate) fn selection_mask(doc: &Document) -> Result<Option<LayerMask>> {
    doc.selection.as_ref().map(|sel| mask_at_depth(LayerMask { surface: sel.clone(), ..LayerMask::reveal_all() }, doc.depth)).transpose()
}

/// New pixel masks use the document's depth, as PSD saves/imports do. Otherwise a later soft
/// brush stroke quantizes differently depending on whether the document reopened as PSD or
/// native. Converting a selection keeps its coverage and does not alter the selection itself.
fn mask_at_depth(mut mask: LayerMask, depth: SampleType) -> Result<LayerMask> {
    let format = mask.surface.format().with_sample(depth);
    if mask.surface.format() != format {
        mask.surface = mask.surface.try_convert(format)?;
    }
    Ok(mask)
}

/// Mask a fill or adjustment layer being created, as Photoshop does: the path selected in the
/// Paths panel (`"path"`: `"work"`, a saved path's name or a path object) becomes its vector
/// mask, which makes a Solid Color fill a shape (#1419); without one, the selection becomes its
/// layer mask ([`selection_mask`]); without either, a white mask reveals the whole layer (#1768).
/// A path that isn't there is an error.
pub(crate) fn mask_new_layer(doc: &Document, l: &mut Layer, p: &Value, cmd: &str) -> Result<()> {
    let path = match p.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(n)) if crate::vector_cmds::is_work(Some(n)) => Some(doc.work_path.clone().ok_or_else(|| bad(cmd, "no work path"))?),
        Some(Value::String(n)) => {
            Some(doc.paths.iter().find(|sp| sp.name == *n).map(|sp| sp.path.clone()).ok_or_else(|| bad(cmd, format!("no path named \"{n}\"")))?)
        }
        Some(v) => Some(crate::vector_cmds::parse_path(v).map_err(|e| bad(cmd, e))?),
    };
    match path {
        Some(path) => l.vector_mask = Some(photocraft_doc::VectorMask::new(path)),
        None => l.mask = Some(mask_at_depth(selection_mask(doc)?.unwrap_or_else(LayerMask::reveal_all), doc.depth)?),
    }
    Ok(())
}

/// The `"path"` param of the new fill and adjustment layer commands ([`mask_new_layer`]).
pub(crate) const NEW_LAYER_PATH: &str =
    r##""path":"work"|saved path name|{…path}? (the active path: becomes the vector mask; else the selection is the layer mask, or white with no selection)"##;

fn new_adjustment(s: &mut Session, adj: Adjustment, p: &Value, cmd: &str) -> Result<Value> {
    let label = format!("New {} Layer", adj.label());
    let name = adj.label().to_string();
    let id = s.edit(&label, |doc, active| {
        let mut l = Layer::new(doc.next_layer_name(&name), LayerContent::Adjustment(adj));
        mask_new_layer(doc, &mut l, p, cmd)?;
        let id = doc.insert_above(*active, l);
        *active = Some(id);
        Ok(id)
    })?;
    Ok(json!({ "layer": id.0 }))
}

fn destructive_adjust(s: &mut Session, label: &str, adj: Adjustment, p: &Value) -> Result<Value> {
    if is_mask_target(p) {
        // The targeted layer mask (#780): ⌘I inverts it, as in Photoshop.
        let id = layer_param(s, &Value::Null)?;
        return s.edit(label, |doc, _| {
            let sel = doc.selection.clone();
            let l = doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?;
            let mask = l.mask.as_mut().ok_or_else(|| EngineError::Other("layer has no mask".into()))?;
            pixels::adjust_mask(&mut mask.surface, &adj, sel.as_ref());
            Ok(Value::Null)
        });
    }
    if crate::channel_cmds::is_channel_target(p) {
        // Alpha channel / Quick Mask target: the adjustment runs on the grayscale channel as on
        // a layer mask, its unstored black included, so ⌘I in Quick Mask inverts it all (#6095).
        return s.edit(label, |doc, _| {
            let sel = doc.selection.clone();
            if let Some(surf) = crate::channel_cmds::channel_surface_for_filter(doc, None, p)? {
                pixels::adjust_mask(surf, &adj, sel.as_ref());
            }
            Ok(Value::Null)
        });
    }
    let id = layer_param(s, &Value::Null)?;
    s.edit(label, |doc, _| {
        let sel = doc.selection.clone();
        let mode = doc.mode;
        let surf = paint_surface(doc, id, &Value::Null)?;
        pixels::adjust_surface(surf, &adj, sel.as_ref(), mode);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Lenient adjustment from params for previews: bad params fall back to the kind's defaults.
/// Commands use the checked [`crate::adjust_params::from_params`].
pub fn adjustment_from_params(kind: &str, p: &Value) -> Adjustment {
    crate::adjust_params::from_params(kind, p, None, ColorMode::Rgb)
        .or_else(|_| crate::adjust_params::default_for(kind, ColorMode::Rgb))
        .unwrap_or(Adjustment::Invert)
}

fn doc_mode(s: &Session) -> ColorMode {
    s.active().map_or(ColorMode::Rgb, |d| d.doc.mode)
}

/// The colour mode a destructive adjustment parses its tone space in: a targeted layer mask,
/// saved channel or Quick Mask holds gray levels whatever the document's mode, so Curves and
/// Levels stay in the neutral space instead of Lab (where the composite is ignored) or CMYK
/// (where the curve runs on the inks) (#5335).
fn adjust_mode(s: &Session, p: &Value) -> ColorMode {
    if is_mask_target(p) || crate::channel_cmds::is_channel_target(p) { ColorMode::Grayscale } else { doc_mode(s) }
}

/// Tone params for a gray plane target: the document-space channel records are dropped, because
/// those keys select the Lab or CMYK tone space and a plane only holds levels (#5335).
fn gray_tone_params(p: &Value) -> Value {
    match p {
        Value::Object(m) => {
            let mut o = Map::new();
            for (k, v) in m {
                if !matches!(k.as_str(), "cyan" | "magenta" | "yellow" | "black" | "lightness" | "a" | "b") {
                    o.insert(k.clone(), v.clone());
                }
            }
            Value::Object(o)
        }
        other => other.clone(),
    }
}

// ---------- the table ----------

const SPECS: &[fn() -> Vec<CommandSpec>] = &[
    file_menu::specs,
    edit_menu::specs,
    select_menu::specs,
    layer_menu::specs,
    image_menu::specs,
    paint::specs,
    queries::specs,
    adjustments::specs,
    crate::layer_style::specs,
    crate::layer_list_cmds::specs,
    crate::filters::specs,
    crate::filters_ext::specs,
    crate::color_to_alpha_cmds::specs,
    crate::gallery_cmds::specs,
    crate::gradient_fill_cmds::specs,
    crate::solid_fill_cmds::specs,
    crate::pattern_fill_cmds::specs,
    crate::type_cmds::specs,
    crate::type_mask_cmds::specs,
    crate::transform_cmds::specs,
    crate::float_cmds::specs,
    crate::vector_cmds::specs,
    crate::path_edit_cmds::specs,
    crate::smartselect_cmds::specs,
    crate::cutout_cmds::specs,
    crate::symmetry_cmds::specs,
    crate::edit_cmds::specs,
    crate::color_cmds::specs,
    crate::brush_cmds::specs,
    crate::brush_preset_cmds::specs,
    crate::eraser_cmds::specs,
    crate::preset_import_cmds::specs,
    crate::atn_cmds::specs,
    crate::actions_edit_cmds::specs,
    crate::swatch_cmds::specs,
    crate::retouch_cmds::specs,
    crate::redeye_cmds::specs,
    crate::image_cmds::specs,
    crate::perspective_crop_cmds::specs,
    crate::selection_cmds::specs,
    crate::magnetic_cmds::specs,
    crate::select_extra_cmds::specs,
    crate::paint_cmds::specs,
    crate::extra_cmds::specs,
    crate::background_cmds::specs,
    crate::file_cmds::specs,
    crate::exr_cmds::specs,
    crate::type_extra_cmds::specs,
    crate::type_styles_cmds::specs,
    crate::type_spell_cmds::specs,
    crate::type_caret_cmds::specs,
    crate::smart_cmds::specs,
    crate::layer_multi_cmds::specs,
    crate::layer_copy_cmds::specs,
    crate::prefs::specs,
    crate::edit_menu_cmds::specs,
    crate::fill_key_cmds::specs,
    crate::sample_cmds::specs,
    crate::face_cmds::specs,
    crate::sampler_cmds::specs,
    crate::brush_key_cmds::specs,
    crate::stamp_cmds::specs,
    crate::align_cmds::specs,
    crate::photo_cmds::specs,
    crate::lens_cmds::specs,
    crate::vp_cmds::specs,
    crate::channel_cmds::specs,
    crate::adjust_cmds::specs,
    crate::layer_menu_cmds::specs,
    crate::layer_label_cmds::specs,
    crate::mode_cmds::specs,
    crate::multichannel_cmds::specs,
    crate::pattern_cmds::specs,
    crate::warp_cmds::specs,
    crate::comps_cmds::specs,
    crate::artboard_cmds::specs,
    crate::distort_cmds::specs,
    crate::analysis_cmds::specs,
    crate::notes_cmds::specs,
    crate::history_cmds::specs,
    crate::history_select_cmds::specs,
    crate::proof_sim::specs,
    crate::presets::specs,
    crate::document_preset_cmds::specs,
    crate::export_preset_cmds::specs,
    crate::render_cmds::specs,
    crate::slice_cmds::specs,
    crate::web_cmds::specs,
    crate::automate_cmds::specs,
    crate::print_cmds::specs,
    crate::installed_profiles::specs,
    crate::pick_cmds::specs,
    crate::layer_nav_cmds::specs,
    crate::frame_cmds::specs,
    crate::migrate_cmds::specs,
    crate::trap_cmds::specs,
    crate::timeline_cmds::specs,
    crate::video_cmds::specs,
    crate::jobs::specs,
    crate::wia_cmds::specs,
    crate::rasterize_style_cmds::specs,
    crate::variables_cmds::specs,
    crate::plugin_cmds::specs,
    crate::group_view_cmds::specs,
    crate::fx_view_cmds::specs,
    crate::fx_visibility_cmds::specs,
    crate::mask_view_cmds::specs,
    crate::layer_mask_props_cmds::specs,
    crate::actions_cmds::specs,
];

fn build() -> Vec<CommandSpec> {
    let specs = SPECS.iter().flat_map(|specs| specs());
    // The dispatch guard's unit test needs a command that panics on purpose (REL-3).
    #[cfg(test)]
    let specs = specs.chain(std::iter::once(test_panic_command()));
    specs.collect()
}

/// A command whose `run` panics on purpose, so the last-resort guard in `jobs::dispatch`
/// can be tested (REL-3). Test builds only: `commands::build` pushes it under `cfg(test)`.
#[cfg(test)]
fn test_panic_command() -> CommandSpec {
    #[allow(clippy::panic)]
    fn run(_: &mut Session, _: &Value) -> Result<Value> {
        panic!("test: this command panicked on purpose")
    }
    CommandSpec { id: "test.panic", label: "Test Panic", menu: &[], shortcut: None, params: r##"{}"##, enabled: |_: &Session| Ok(()), run, journal: false }
}

/// Commands generated from a table share one `run` fn; the kind is recovered from the id.
pub(crate) fn inject_kind(id: &str, params: Value) -> Value {
    let kind = id.strip_prefix("layer.newAdjustmentLayer.").or_else(|| id.strip_prefix("image.adjustments."));
    match (kind, params) {
        (Some(k), Value::Object(mut m)) => {
            m.insert("__kind".into(), Value::String(k.into()));
            Value::Object(m)
        }
        (Some(k), _) => json!({ "__kind": k }),
        (None, p) => p,
    }
}

pub fn adjustment_kind(a: &Adjustment) -> &'static str {
    match a {
        Adjustment::BrightnessContrast { .. } => "brightnessContrast",
        Adjustment::Levels { .. } => "levels",
        Adjustment::Curves { .. } => "curves",
        Adjustment::Exposure { .. } => "exposure",
        Adjustment::Vibrance { .. } => "vibrance",
        Adjustment::HueSaturation { .. } => "hueSaturation",
        Adjustment::ColorBalance { .. } => "colorBalance",
        Adjustment::BlackWhite { .. } => "blackWhite",
        Adjustment::PhotoFilter { .. } => "photoFilter",
        Adjustment::ChannelMixer { .. } => "channelMixer",
        Adjustment::Posterize { .. } => "posterize",
        Adjustment::Threshold { .. } => "threshold",
        Adjustment::GradientMap { .. } => "gradientMap",
        Adjustment::SelectiveColor { .. } => "selectiveColor",
        Adjustment::ColorLookup { .. } => "colorLookup",
        _ => "invert",
    }
}

fn arrange(s: &mut Session, p: &Value, delta: i32) -> Result<Value> {
    let id = layer_param(s, p)?;
    s.edit("Arrange", |doc, _| {
        // The Background stays at the bottom: it doesn't move and nothing goes below it.
        let bg = crate::background_cmds::background_id(doc);
        if bg == Some(id) {
            return Err(EngineError::Other("the Background layer can't be moved".into()));
        }
        let steps = match delta {
            i32::MAX => 10_000,
            i32::MIN => -10_000,
            d => d,
        };
        let dir = steps.signum();
        let before = crate::clip_group::Before::read(doc, &[id]);
        let mut moved = false;
        for _ in 0..steps.abs() {
            if !doc.shift(id, dir) {
                break;
            }
            if crate::background_cmds::keep_background_at_bottom(bg, doc).is_err() {
                doc.shift(id, -dir);
                break;
            }
            moved = true;
        }
        if !moved {
            return Err(EngineError::Other("layer can't move further".into()));
        }
        crate::clip_group::settle(doc, &[id], &before, false);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Add (`Some`) or remove (`None`) a layer mask. Adding acts on every selected layer that can take
/// one ([`crate::layer_multi_cmds::mask_targets`]), in one history step.
fn set_mask(s: &mut Session, p: &Value, label: &str, mask: Option<LayerMask>) -> Result<Value> {
    set_mask_with(s, p, label, mask, false)
}

/// [`set_mask`] that also drops the selection the mask was made from, in the same history step.
fn set_mask_and_deselect(s: &mut Session, p: &Value, label: &str, mask: LayerMask) -> Result<Value> {
    set_mask_with(s, p, label, Some(mask), true)
}

fn set_mask_with(s: &mut Session, p: &Value, label: &str, mask: Option<LayerMask>, deselect: bool) -> Result<Value> {
    let ids = if mask.is_some() { crate::layer_multi_cmds::mask_targets(s, p)? } else { vec![layer_param(s, p)?] };
    s.edit(label, |doc, _| {
        let mask = mask.clone().map(|mask| mask_at_depth(mask, doc.depth)).transpose()?;
        for id in &ids {
            if mask.is_some() {
                crate::extra_cmds::background_to_layer_for_mask(doc, *id);
            }
            doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?.mask = mask.clone();
        }
        if deselect {
            doc.selection = None;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Move a layer's pixels, linked mask and type by whole pixels (vectors move via
/// `vector_cmds::translate_vectors`).
pub(crate) fn translate_layer(doc: &Document, l: &mut Layer, dx: i32, dy: i32) {
    use crate::layer_multi_cmds::shift_surface as translate_surface;
    // Linked patterns in the layer's effects move with it.
    if let Some(r) = &mut l.effects.reference {
        *r = (r.0 + f64::from(dx), r.1 + f64::from(dy));
    }
    if let Some(m) = &mut l.mask
        && m.linked
    {
        m.surface = translate_surface(&m.surface, dx, dy);
    }
    match &mut l.content {
        LayerContent::Raster(s) => *s = translate_surface(s, dx, dy),
        LayerContent::Text(t) => {
            t.transform = photocraft_geom::Affine::translate(dx as f64, dy as f64).mul(&t.transform);
            // A whole-pixel move shifts the rendered text and its PSD position as they are.
            // Re-rendering would replace Photoshop's rendering of imported type, and drop the
            // text when its font is missing (artboard export, moving type with the Move tool).
            let raw = t.psd_raw.as_deref().map(|r| photocraft_text::psd::translate_tysh(r, f64::from(dx), f64::from(dy)));
            match (&mut t.cache, raw) {
                (Some(c), Some(Some(r))) => {
                    *c = translate_surface(c, dx, dy);
                    t.psd_raw = Some(std::sync::Arc::new(r));
                }
                (Some(c), None) => *c = translate_surface(c, dx, dy),
                _ => crate::type_cmds::refresh(doc, t),
            }
        }
        LayerContent::Group(g) => {
            // Moving an artboard moves the board with its contents.
            if let Some(a) = &mut g.artboard {
                a.rect = a.rect.translate(dx, dy);
            }
            for c in &mut g.children {
                translate_layer(doc, c, dx, dy);
            }
        }
        LayerContent::Smart(sm) => crate::smart_cmds::shift_smart(sm, dx, dy),
        _ => {}
    }
}

/// A copy of layer `id` as Layer › Duplicate Layer makes it, not yet in the document: a fresh id,
/// "… copy" appended to the name, and a copy of the Background layer is an ordinary, unlocked
/// layer (Photoshop).
pub(crate) fn layer_copy(doc: &Document, id: LayerId) -> Result<Layer> {
    let src = doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    // A copy of the Background layer is an ordinary, unlocked layer (Photoshop).
    let from_background = doc.is_background(id);
    let mut dup = src.duplicate();
    dup.name = doc.copy_name(&dup.name);
    if from_background {
        dup.locks = Default::default();
    }
    Ok(dup)
}

/// Layer › Merge Group (⌘E with a group selected, #3832): the group's visible contents, as they
/// show, become one pixel layer in its place, named after it. Like Photoshop, the layer keeps the
/// group's opacity, blend mode (Pass Through becomes Normal), clipping and locks; the group's mask
/// and style are applied to the pixels.
fn merge_group(s: &mut Session, id: LayerId) -> Result<Value> {
    s.edit("Merge Group", |doc, active| {
        let group = doc.layer(id).ok_or(EngineError::NoLayer(id))?.clone();
        if !group.visible {
            return Err(EngineError::Other("the group is hidden".into()));
        }
        let fmt = doc.pixel_format();
        let fmt = photocraft_color::PixelFormat::new(fmt.mode, fmt.sample, true);
        let mut solo = group.clone();
        solo.opacity = 1.0;
        solo.blend = photocraft_doc::BlendMode::Normal;
        solo.clipped = false;
        let mut tmp = doc.clone();
        tmp.layers = vec![solo];
        let mut merged = Layer::new(group.name.clone(), LayerContent::Raster(pixels::composite_layers_uncropped(&tmp, fmt)));
        merged.id = group.id;
        merged.opacity = group.opacity;
        merged.blend = if group.blend == photocraft_doc::BlendMode::PassThrough { photocraft_doc::BlendMode::Normal } else { group.blend };
        merged.clipped = group.clipped;
        merged.locks = group.locks;
        *doc.layer_mut(id).ok_or(EngineError::NoLayer(id))? = merged;
        *active = Some(id);
        Ok(())
    })?;
    Ok(json!({ "layer": id.0 }))
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn duplicate_ids_keep_the_first_spec_and_listing_order() {
        let mut built = build().into_iter();
        let first = built.next().unwrap();
        let mut second = built.next().unwrap();
        let id = first.id;
        second.id = id;
        let registry = Registry::new(vec![first, second]);
        assert_eq!(registry.by_id.get(id), Some(&0));
        assert_eq!(registry.specs.len(), 2);
    }
}

#[cfg(test)]
mod set_props_tests {
    use super::*;

    #[test]
    fn opacity_and_fill_outside_zero_to_one_are_refused_not_clamped() {
        // #5242: `55` (a percentage, as the Layer Style commands take) used to become 100 %.
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("layer.setProps", json!({"opacity": 0.55, "fill": 0.4})).unwrap();
        let layer = |s: &Session| {
            let d = s.active().unwrap();
            let l = d.doc.layer(d.active_layer.unwrap()).unwrap();
            (l.opacity, l.fill_opacity)
        };
        assert_eq!(layer(&s), (0.55, 0.4));
        for bad in [json!({"opacity": 55}), json!({"opacity": -0.1}), json!({"fill": 1.5}), json!({"opacity": "50%"}), json!({"opacity": 0.5, "fill": 40})] {
            let e = s.execute("layer.setProps", bad.clone()).unwrap_err().to_string();
            assert!(e.contains("0..1"), "{bad}: {e}");
            assert_eq!(layer(&s), (0.55, 0.4), "{bad} left the layer alone");
        }
        s.execute("layer.setProps", json!({"opacity": 0, "fill": 1})).unwrap();
        assert_eq!(layer(&s), (0.0, 1.0));
    }
}
