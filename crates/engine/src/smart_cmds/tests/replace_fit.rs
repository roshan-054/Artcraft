//! Replace Contents with `fit: "stretch"`: a file of another size fills the old contents' box,
//! through the same transform, warp or perspective, as a file of the old size would; with
//! `fit: "contain"` it is scaled to fit inside that box, keeping its aspect, and centred.

use super::*;
use photocraft_geom::warp::{BezierMesh, Warp};

/// A solid PNG of `w × h` written to `dir/name`.
fn png_file(dir: &std::path::Path, name: &str, w: u32, h: u32, rgb: [f32; 3]) -> String {
    let doc = Document::with_background(
        "p",
        Size::new(w, h),
        photocraft_color::ColorMode::Rgb,
        photocraft_color::SampleType::U8,
        photocraft_color::Color::rgba(rgb[0], rgb[1], rgb[2], 1.0),
    );
    let path = dir.join(name);
    std::fs::write(&path, photocraft_io::export(&doc, "png", &Default::default()).unwrap().bytes).unwrap();
    path.to_str().unwrap().to_string()
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pcraft-fit-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A converted smart object (painted block, contents 46 × 33) and its layer id.
fn smart_session() -> (Session, LayerId) {
    let mut s = session(8);
    paint(&mut s);
    let id = LayerId(convert(&mut s));
    (s, id)
}

/// The old contents' box in source pixels.
fn old_box(s: &Session, id: LayerId) -> [f64; 4] {
    let doc = &s.active().unwrap().doc;
    source_box(doc, smart(doc, id).unwrap()).unwrap()
}

/// Sets the placement of `id` with `f` and re-renders it.
fn set_placement(s: &mut Session, id: LayerId, f: impl FnOnce(&mut SmartObject)) {
    s.edit("setup", |doc, _| {
        f(smart_mut(doc, id)?);
        refresh_or_fail(doc, id)
    })
    .unwrap();
}

fn rendered(s: &Session, id: LayerId) -> Surface {
    let doc = &s.active().unwrap().doc;
    smart(doc, id).unwrap().cache.clone().unwrap()
}

fn replace(s: &mut Session, id: LayerId, path: &str, fit: Option<&str>) {
    let mut p = json!({"layer": id.0, "path": path});
    if let Some(f) = fit {
        p["fit"] = json!(f);
    }
    s.execute("layer.smartObjects.replaceContents", p).unwrap();
}

/// Mean |alpha difference| over the union of both renders' bounds.
fn alpha_diff(a: &Surface, b: &Surface) -> f32 {
    let r = a.content_bounds().union(&b.content_bounds());
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            sum += (a.rgba(x, y)[3] - b.rgba(x, y)[3]).abs();
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

fn near(a: Rect, b: Rect, tol: i32) -> bool {
    (a.x0 - b.x0).abs() <= tol && (a.y0 - b.y0).abs() <= tol && (a.x1 - b.x1).abs() <= tol && (a.y1 - b.y1).abs() <= tol
}

/// Renders the same placement twice: once from a full-size file (`fit` keep), once from a
/// smaller file with `fit: "stretch"`. Both must cover the same pixels.
fn full_vs_stretched(tag: &str, setup: impl Fn(&mut SmartObject, [f64; 4])) {
    let dir = tmp_dir(tag);
    let (mut full, id) = smart_session();
    let b = old_box(&full, id);
    let (w, h) = ((b[2] - b[0]) as u32, (b[3] - b[1]) as u32);
    set_placement(&mut full, id, |sm| setup(sm, b));
    replace(&mut full, id, &png_file(&dir, "full.png", w, h, [1.0, 0.0, 0.0]), None);
    let (mut small, id2) = smart_session();
    set_placement(&mut small, id2, |sm| setup(sm, b));
    replace(&mut small, id2, &png_file(&dir, "small.png", w.div_ceil(2), h.div_ceil(2), [1.0, 0.0, 0.0]), Some("stretch"));
    let (a, c) = (rendered(&full, id), rendered(&small, id2));
    assert!(near(a.content_bounds(), c.content_bounds(), 1), "{tag}: {:?} vs {:?}", a.content_bounds(), c.content_bounds());
    assert!(alpha_diff(&a, &c) < 0.03, "{tag}: alpha diff {}", alpha_diff(&a, &c));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stretch_fills_the_old_box_through_an_affine_transform() {
    full_vs_stretched("affine", |sm, _| sm.transform = Affine { m: [1.5, 0.2, -0.1, 1.2, 3.0, 2.0] });
}

#[test]
fn stretch_fills_the_old_box_through_a_custom_warp() {
    full_vs_stretched("warp", |sm, b| {
        // A bulge: the middle control points pushed out.
        let mesh = BezierMesh::identity(b, 1, 1).map_points(|p| {
            let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
            [p[0] + 0.15 * (p[0] - cx), p[1] + 0.25 * (p[1] - cy) * ((p[0] - cx).abs() / (b[2] - b[0]))]
        });
        sm.warp = Some(Warp::custom(mesh, b));
    });
}

#[test]
fn stretch_fills_the_old_box_through_a_warp_over_a_wider_box() {
    // The warp's box need not be the contents' box (it may spill past it).
    full_vs_stretched("warp-wide", |sm, b| {
        let wide = [b[0] - 4.0, b[1] - 2.0, b[2] + 6.0, b[3] + 3.0];
        let mesh = BezierMesh::identity(wide, 2, 1).map_points(|p| [p[0] * 0.9 + 0.05 * p[1], p[1]]);
        sm.warp = Some(Warp::custom(mesh, wide));
    });
}

#[test]
fn stretch_fills_the_old_box_through_a_preset_warp() {
    full_vs_stretched("preset", |sm, b| sm.warp = Some(Warp::preset(photocraft_geom::warp::WarpStyle::Arc, 30.0, b)));
}

#[test]
fn stretch_fills_the_old_box_through_a_perspective() {
    full_vs_stretched("perspective", |sm, _| sm.perspective = Some([1.0, 0.1, -6.0, 0.05, 1.1, 5.0, 0.002, 0.001, 1.0]));
}

#[test]
fn keep_is_the_default_and_keeps_the_pixel_size() {
    let dir = tmp_dir("keep");
    let (mut s, id) = smart_session();
    let before = rendered(&s, id).content_bounds();
    replace(&mut s, id, &png_file(&dir, "small.png", 4, 4, [0.0, 0.0, 1.0]), None);
    let keep = rendered(&s, id).content_bounds();
    assert_eq!(keep, Rect::new(before.x0, before.y0, before.x0 + 4, before.y0 + 4));
    s.execute("edit.undo", json!({})).unwrap();
    replace(&mut s, id, &png_file(&dir, "small2.png", 4, 4, [0.0, 0.0, 1.0]), Some("keep"));
    assert_eq!(rendered(&s, id).content_bounds(), keep);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stretch_is_one_undo_step() {
    let dir = tmp_dir("undo");
    let (mut s, id) = smart_session();
    let before = active_smart(&s);
    replace(&mut s, id, &png_file(&dir, "small.png", 5, 7, [0.0, 1.0, 0.0]), Some("stretch"));
    assert_ne!(active_smart(&s).transform, before.transform, "the placement was re-aimed");
    s.execute("edit.undo", json!({})).unwrap();
    let after = active_smart(&s);
    assert_eq!(after.transform, before.transform);
    assert_eq!(after.cache.unwrap().content_bounds(), before.cache.unwrap().content_bounds());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stretch_with_a_same_size_file_changes_no_placement() {
    let dir = tmp_dir("same");
    let (mut s, id) = smart_session();
    let b = old_box(&s, id);
    let before = active_smart(&s);
    replace(&mut s, id, &png_file(&dir, "same.png", (b[2] - b[0]) as u32, (b[3] - b[1]) as u32, [0.0, 1.0, 0.0]), Some("stretch"));
    assert_eq!(active_smart(&s).transform, before.transform);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn contain_fits_the_file_inside_the_old_box_keeping_its_aspect() {
    // A square file into the 46 × 33 box: scaled to 33 × 33 and centred across the width, the
    // rest of the box left empty.
    let dir = tmp_dir("contain");
    let (mut s, id) = smart_session();
    let before = rendered(&s, id).content_bounds();
    let b = old_box(&s, id);
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    replace(&mut s, id, &png_file(&dir, "square.png", 10, 10, [0.0, 1.0, 0.0]), Some("contain"));
    let k = (bw / 10.0).min(bh / 10.0);
    let (px, py) = (((bw - 10.0 * k) / 2.0).round() as i32, ((bh - 10.0 * k) / 2.0).round() as i32);
    let want = Rect::new(before.x0 + px, before.y0 + py, before.x1 - px, before.y1 - py);
    let got = rendered(&s, id).content_bounds();
    assert!(near(got, want, 1), "{got:?} vs {want:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn contain_with_the_boxs_own_aspect_places_the_file_like_stretch() {
    let dir = tmp_dir("contain-aspect");
    let (mut stretched, id) = smart_session();
    let b = old_box(&stretched, id);
    let (w, h) = (2 * (b[2] - b[0]) as u32, 2 * (b[3] - b[1]) as u32);
    replace(&mut stretched, id, &png_file(&dir, "a.png", w, h, [1.0, 0.0, 0.0]), Some("stretch"));
    let (mut contained, id2) = smart_session();
    replace(&mut contained, id2, &png_file(&dir, "b.png", w, h, [1.0, 0.0, 0.0]), Some("contain"));
    assert_eq!(active_smart(&contained).transform, active_smart(&stretched).transform);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn contain_is_one_undo_step() {
    let dir = tmp_dir("contain-undo");
    let (mut s, id) = smart_session();
    let before = active_smart(&s);
    replace(&mut s, id, &png_file(&dir, "small.png", 5, 7, [0.0, 1.0, 0.0]), Some("contain"));
    assert_ne!(active_smart(&s).transform, before.transform, "the placement was re-aimed");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(active_smart(&s).transform, before.transform);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unknown_fit_is_refused_before_anything_changes() {
    let dir = tmp_dir("bad");
    let (mut s, id) = smart_session();
    let before = active_smart(&s);
    let p = json!({"layer": id.0, "path": png_file(&dir, "x.png", 3, 3, [1.0, 1.0, 1.0]), "fit": "cover"});
    let e = s.execute("layer.smartObjects.replaceContents", p).unwrap_err().to_string();
    assert!(e.contains("fit"), "{e}");
    assert_eq!(active_smart(&s).source, before.source);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stretch_reads_a_psd_layers_box_from_its_stored_size_without_decoding_the_old_contents() {
    // A PSD placed layer whose old contents can't be read (a missing link): its soLD still
    // stores the 10 × 8 size the placement (2×, at 20, 10) was authored for.
    let dir = tmp_dir("sold");
    let mut s = session(8);
    let spec = photocraft_io::smart_map::PlacedSpec {
        idnt: "uuid-gone",
        placed: "uuid-gone",
        transform: Affine { m: [2.0, 0.0, 0.0, 2.0, 20.0, 10.0] },
        perspective: None,
        size: (10.0, 8.0),
        dpi: 72.0,
        warp: None,
        filter_fx: None,
    };
    let sold = photocraft_io::smart_map::sold_bytes(None, &spec, &mut Vec::new());
    let id = s
        .edit("place", |doc, active| {
            let mut sm = SmartObject::new(SmartSource::Linked { path: "uuid-gone".into() }, spec.transform, None);
            sm.psd_raw = Some(Arc::new(sold.clone()));
            let l = Layer::new("placed", LayerContent::Smart(sm));
            let id = doc.insert_above(*active, l);
            *active = Some(id);
            Ok(id)
        })
        .unwrap();
    replace(&mut s, id, &png_file(&dir, "small.png", 5, 4, [1.0, 0.0, 0.0]), Some("stretch"));
    assert_eq!(rendered(&s, id).content_bounds(), Rect::new(20, 10, 40, 26));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clip_to_canvas_renders_only_the_canvas_and_the_same_pixels_inside_it() {
    let dir = tmp_dir("clip");
    let design = png_file(&dir, "d.png", 30, 20, [0.2, 0.7, 0.4]);
    // Placed 3× and bent, so the warped contents spill far past the 64 × 48 canvas.
    let setup = |sm: &mut SmartObject, b: [f64; 4]| {
        sm.transform = Affine { m: [3.0, 0.0, 0.0, 3.0, -20.0, -10.0] };
        let mesh = BezierMesh::identity(b, 1, 1).map_points(|p| [p[0] + 0.1 * p[1], p[1] + 3.0 * (p[0] / (b[2] - b[0]) * 3.0).sin()]);
        sm.warp = Some(Warp::custom(mesh, b));
    };
    let (mut full, id) = smart_session();
    let b = old_box(&full, id);
    set_placement(&mut full, id, |sm| setup(sm, b));
    replace_contents_with(&mut full, id, "d.png", std::fs::read(&design).unwrap(), Fit::Stretch, false).unwrap();
    let (mut clipped, id2) = smart_session();
    set_placement(&mut clipped, id2, |sm| setup(sm, b));
    replace_contents_with(&mut clipped, id2, "d.png", std::fs::read(&design).unwrap(), Fit::Stretch, true).unwrap();
    let canvas = clipped.active().unwrap().doc.bounds();
    let (a, c) = (rendered(&full, id), rendered(&clipped, id2));
    assert!(!canvas.contains_rect(&a.content_bounds()), "the full render spills: {:?}", a.content_bounds());
    assert!(canvas.contains_rect(&c.content_bounds()), "clipped: {:?}", c.content_bounds());
    let inside = a.content_bounds().intersect(&canvas);
    assert_eq!(c.read_region(inside), a.read_region(inside));
    // The flattened documents are the same.
    assert_eq!(flat(&full), flat(&clipped));
    // A later unclipped render of the same file is whole again (clipped renders are cached apart).
    clipped.execute("edit.undo", json!({})).unwrap();
    replace_contents_with(&mut clipped, id2, "d.png", std::fs::read(&design).unwrap(), Fit::Stretch, false).unwrap();
    assert_eq!(rendered(&clipped, id2).content_bounds(), a.content_bounds());
    std::fs::remove_dir_all(&dir).ok();
}

/// Full and clipped renders of a placement spilling past the canvas, with `extra` applied to the
/// layer first; returns both flattened documents.
fn full_and_clipped_flat(tag: &str, extra: impl Fn(&mut Session, LayerId)) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
    let dir = tmp_dir(tag);
    let design = png_file(&dir, "d.png", 30, 20, [0.2, 0.7, 0.4]);
    let setup = |sm: &mut SmartObject, b: [f64; 4]| {
        sm.transform = Affine { m: [3.0, 0.0, 0.0, 3.0, -20.0, -10.0] };
        let mesh = BezierMesh::identity(b, 1, 1).map_points(|p| [p[0] + 0.1 * p[1], p[1]]);
        sm.warp = Some(Warp::custom(mesh, b));
    };
    let mut out = Vec::new();
    for clip in [false, true] {
        let (mut s, id) = smart_session();
        let b = old_box(&s, id);
        set_placement(&mut s, id, |sm| setup(sm, b));
        extra(&mut s, id);
        replace_contents_with(&mut s, id, "d.png", std::fs::read(&design).unwrap(), Fit::Stretch, clip).unwrap();
        out.push(flat(&s));
    }
    std::fs::remove_dir_all(&dir).ok();
    let clipped = out.pop().unwrap();
    (out.pop().unwrap(), clipped)
}

#[test]
fn clip_to_canvas_keeps_the_flattened_result_of_a_layer_with_effects() {
    // A stroke follows the layer's edge: cutting the layer at the canvas would draw it there.
    let (full, clipped) = full_and_clipped_flat("fx", |s, id| {
        s.execute("layer.layerStyle.stroke", json!({"layer": id.0, "size": 3, "position": "inside", "color": "#ff0000"})).unwrap();
    });
    assert_eq!(full, clipped);
}

#[test]
fn clip_to_canvas_keeps_the_flattened_result_of_a_layer_with_smart_filters() {
    // A blur reads past the canvas edge; a clipped layer would repeat the edge instead.
    let (full, clipped) = full_and_clipped_flat("sf", |s, id| {
        s.execute("filter.blur.gaussianBlur", json!({"layer": id.0, "radius": 4})).unwrap();
    });
    assert_eq!(full, clipped);
}

#[test]
fn clip_to_canvas_renders_only_what_the_layer_mask_reveals() {
    // A mask that hides everything but a 10 × 6 strip: the clipped render stays inside the strip
    // (plus the mask's margin) and the flattened document is the same as a full render.
    let mask_strip = |s: &mut Session, id: LayerId| {
        s.edit("mask", |doc, _| {
            let mut m = photocraft_doc::LayerMask::hide_all();
            m.surface.fill_rect(Rect::new(20, 12, 30, 18), &[1.0]);
            doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?.mask = Some(m);
            Ok(())
        })
        .unwrap();
    };
    let (full, clipped) = full_and_clipped_flat("mask", mask_strip);
    assert_eq!(full, clipped);

    let dir = tmp_dir("mask-bounds");
    let (mut s, id) = smart_session();
    let b = old_box(&s, id);
    set_placement(&mut s, id, |sm| {
        sm.transform = Affine { m: [3.0, 0.0, 0.0, 3.0, -20.0, -10.0] };
        sm.warp = Some(Warp::custom(BezierMesh::identity(b, 1, 1).map_points(|p| [p[0] + 0.1 * p[1], p[1]]), b));
    });
    mask_strip(&mut s, id);
    replace_contents_with(&mut s, id, "d.png", std::fs::read(png_file(&dir, "d.png", 30, 20, [0.2, 0.7, 0.4])).unwrap(), Fit::Stretch, true).unwrap();
    let r = rendered(&s, id).content_bounds();
    assert!(Rect::new(20, 12, 30, 18).inflate(2).contains_rect(&r), "rendered {r:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn content_hash_is_blake3_and_never_stale_for_a_new_buffer() {
    let a = Arc::new(vec![1u8; 4096]);
    assert_eq!(content_hash(&a), *blake3::hash(&a).as_bytes());
    assert_eq!(content_hash(&a), *blake3::hash(&a).as_bytes(), "memoised");
    drop(a);
    // A new buffer (possibly at the freed address) must be hashed, not served the old digest.
    let b = Arc::new(vec![2u8; 4096]);
    assert_eq!(content_hash(&b), *blake3::hash(&b).as_bytes());
}

#[test]
fn replacing_shared_contents_hashes_the_new_file_once_for_all_instances() {
    // Three instances of one smart object: the new file is one source for all of them.
    let dir = tmp_dir("hash-once");
    let (mut s, id) = smart_session();
    for _ in 0..2 {
        s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
    }
    let before = HASHES.with(std::cell::Cell::get);
    replace(&mut s, id, &png_file(&dir, "big.png", 300, 200, [0.5, 0.2, 0.9]), Some("stretch"));
    let hashed = HASHES.with(std::cell::Cell::get) - before;
    assert_eq!(active_smart(&s).contents_id, smart(&s.active().unwrap().doc, id).unwrap().contents_id);
    assert!(hashed <= 1, "hashed the new file {hashed} times");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn replacing_contents_decodes_the_new_file_once() {
    // The file is decoded to check it and to find its box; the re-render reuses that image.
    let _cache = SOURCE_CACHE_TESTS.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tmp_dir("decode-once");
    let (mut s, id) = smart_session();
    s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
    let before = DECODES.with(std::cell::Cell::get);
    replace(&mut s, id, &png_file(&dir, "d.png", 120, 80, [0.3, 0.6, 0.9]), Some("stretch"));
    let decoded = DECODES.with(std::cell::Cell::get) - before;
    assert_eq!(decoded, 1, "decoded the new file {decoded} times");
    std::fs::remove_dir_all(&dir).ok();
}

fn hide(s: &mut Session, id: LayerId) {
    s.edit("hide", |doc, _| {
        doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?.visible = false;
        Ok(())
    })
    .unwrap();
}

/// Moves `id` into a new hidden group at the top of the document.
fn hide_in_group(s: &mut Session, id: LayerId) {
    s.edit("group", |doc, _| {
        let path = doc.path_of(id).ok_or(EngineError::NoLayer(id))?;
        let [i] = path[..] else { return Err(other("expected a top-level layer")) };
        let layer = doc.layers.remove(i);
        let mut g = Layer::group("G", vec![layer]);
        g.visible = false;
        doc.layers.push(g);
        Ok(())
    })
    .unwrap();
}

#[test]
fn clip_to_canvas_leaves_hidden_instances_unrendered() {
    // Mockups keep a hidden "design here" layer whose copies show the design. The hidden one is
    // not in the flattened document: a clipped replace leaves it unrendered, and the flattened
    // result is the same as a full render.
    let (full, clipped) = full_and_clipped_flat("hidden", |s, id| {
        s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
        hide(s, id);
    });
    assert_eq!(full, clipped);

    let dir = tmp_dir("hidden-cache");
    let design = std::fs::read(png_file(&dir, "d.png", 30, 20, [0.2, 0.7, 0.4])).unwrap();
    for (clip, group) in [(true, false), (true, true), (false, false)] {
        let (mut s, id) = smart_session();
        s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
        if group {
            hide_in_group(&mut s, id)
        } else {
            hide(&mut s, id)
        }
        replace_contents_with(&mut s, id, "d.png", design.clone(), Fit::Stretch, clip).unwrap();
        let doc = &s.active().unwrap().doc;
        let copy = instances(doc, smart(doc, id).unwrap().contents_id).into_iter().find(|&l| l != id).unwrap();
        assert_eq!(smart(doc, id).unwrap().cache.is_none(), clip, "clip {clip}, group {group}: the hidden instance is left unrendered only with clip");
        assert!(
            smart(doc, copy).unwrap().cache.as_ref().is_some_and(|c| !c.content_bounds().is_empty()),
            "clip {clip}, group {group}: the visible copy is rendered"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_banded_source_flatten_gives_the_pixels_of_one_flatten() {
    // Sources are composited in bands to bound memory; the pixels must be those of one flatten.
    use photocraft_color::{ColorMode, SampleType};
    for depth in [SampleType::U8, SampleType::U16, SampleType::F32] {
        banded_flatten_matches_one_flatten(Document::new("d", Size::new(300, 700), ColorMode::Rgb, depth));
    }
}

fn banded_flatten_matches_one_flatten(mut doc: Document) {
    let fmt = doc.pixel_format();
    let mut a = Layer::raster("a", fmt);
    let mut b = Layer::raster("b", fmt);
    b.blend = BlendMode::Multiply;
    b.opacity = 0.6;
    for y in (0..700).step_by(10) {
        for x in (0..300).step_by(10) {
            let (u, v) = (x as f32 / 300.0, y as f32 / 700.0);
            a.surface_mut().unwrap().fill_rect(Rect::new(x, y, x + 10, y + 10), &[u, v, 1.0 - u, (u + v) / 2.0]);
            if (x / 10 + y / 10) % 3 != 0 {
                b.surface_mut().unwrap().fill_rect(Rect::new(x + 3, y, x + 10, y + 7), &[v, 0.5, u, 0.3 + 0.7 * u]);
            }
        }
    }
    doc.layers = vec![a, b];
    let full = buffer_to_surface(&photocraft_compose::flatten(&doc), fmt);
    for band in [300 * 256, 2 << 20] {
        let banded = flatten_banded(&doc, fmt, band);
        assert_eq!(banded.content_bounds(), full.content_bounds(), "{:?}, band {band}", fmt.sample);
        assert_eq!(banded.read_region(doc.bounds()), full.read_region(doc.bounds()), "{:?}, band {band}", fmt.sample);
    }
}

use crate::smart_cmds::stack_cache as stages;

fn tiny_stage(v: f32) -> stages::Stage {
    let fmt = PixelFormat::new(photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8, true);
    let mut surface = Surface::new(fmt);
    surface.fill_rect(Rect::new(0, 0, 4, 4), &[v, v, v, 1.0]);
    stages::Stage { surface, extent: Rect::new(0, 0, 4, 4) }
}

#[test]
fn the_stage_cache_keeps_nothing_at_budget_zero() {
    // A render server sets the budget to 0: each design is rendered once, so its stages would
    // only pile up.
    let mut c = Vec::new();
    stages::insert(&mut c, [1; 32], &tiny_stage(0.5), 0);
    assert!(c.is_empty());
    stages::insert(&mut c, [1; 32], &tiny_stage(0.5), 1 << 20);
    stages::insert(&mut c, [2; 32], &tiny_stage(0.7), 1 << 20);
    assert_eq!(c.len(), 2);
    stages::trim(&mut c, 0);
    assert!(c.is_empty(), "lowering the budget drops what is over it");
}

#[test]
fn the_source_cache_keeps_only_the_newest_entry_at_budget_zero() {
    // The newest decode stays: the same design rendered into the next template isn't decoded again.
    let fmt = PixelFormat::new(photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8, true);
    let entry =
        |k: u8| CacheEntry { key: ([k; 32], fmt), image: SourceImage { surface: Arc::new(Surface::new(fmt)), bounds: Rect::new(0, 0, 10, 10) }, decodes: 1 };
    let mut c = vec![entry(1), entry(2)];
    trim_sources(&mut c, 1 << 20);
    assert_eq!(c.len(), 2);
    trim_sources(&mut c, 0);
    assert_eq!(c.iter().map(|e| e.key.0[0]).collect::<Vec<_>>(), [2]);
}

#[test]
fn performance_prefs_hold_the_smart_object_cache_budgets() {
    let mut s = Session::new();
    let perf = |s: &Session| (s.prefs().performance.smart_source_cache_mb, s.prefs().performance.smart_stage_cache_mb);
    assert_eq!(perf(&s), (1024, 768), "defaults: the budgets the caches had");
    assert_eq!(s.execute("prefs.get", json!({"path": "performance.smartSourceCacheMb"})).unwrap(), json!(1024));
    s.execute("prefs.set", json!({"path": "performance.smartSourceCacheMb", "value": 1024})).unwrap();
    s.execute("prefs.set", json!({"path": "performance.smartStageCacheMb", "value": 768})).unwrap();
    assert!(s.execute("prefs.set", json!({"path": "performance.smartStageCacheMb", "value": -1})).is_err());
    assert_eq!(perf(&s), (1024, 768));
}

#[test]
fn replacing_shared_contents_reduces_the_new_file_once_for_all_instances() {
    // Three instances shrinking a large file through warps: one reduced level serves them all.
    let _cache = SOURCE_CACHE_TESTS.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tmp_dir("level-once");
    let (mut s, id) = smart_session();
    let b = old_box(&s, id);
    set_placement(&mut s, id, |sm| sm.warp = Some(Warp::custom(BezierMesh::identity(b, 1, 1).map_points(|p| [p[0] + 0.1 * p[1], p[1]]), b)));
    for _ in 0..2 {
        s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
    }
    // Each at its own place, so no instance is served another's rendered stage.
    let doc = &s.active().unwrap().doc;
    let copies: Vec<LayerId> = instances(doc, smart(doc, id).unwrap().contents_id).into_iter().filter(|&l| l != id).collect();
    for (k, copy) in copies.into_iter().enumerate() {
        set_placement(&mut s, copy, |sm| sm.transform.m[4] += 3.0 * (k + 1) as f64);
    }
    let big = std::fs::read(png_file(&dir, "big.png", 900, 600, [0.5, 0.2, 0.9])).unwrap();
    let before = photocraft_algo::warp::reduced_levels_built();
    replace_contents_with(&mut s, id, "big.png", big, Fit::Stretch, true).unwrap();
    let built = photocraft_algo::warp::reduced_levels_built() - before;
    assert_eq!(built, 1, "built {built} reduced levels");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_plain_single_layer_source_is_its_own_layer() {
    // A placed PNG or JPEG is one plain layer: flattening it gives that layer back, so the layer
    // is used as is (no composite, no copy). Every alpha against many colours, at 8 and 16 bits;
    // the result must be the flattened surface exactly, transparent pixels and bounds included
    // (a whole-pixel placement copies them, and bounds and some filters read them).
    use photocraft_color::{ColorMode, SampleType};
    let doc_of = |depth: SampleType| {
        let mut doc = Document::new("d", Size::new(256, 300), ColorMode::Rgb, depth);
        let mut l = Layer::raster("a", doc.pixel_format());
        let px: Vec<f32> = (0..300)
            .flat_map(|y| {
                (0..256).flat_map(move |x| {
                    let c = (y % 256) as f32 / 255.0;
                    [c, 1.0 - c, (c * 7.0) % 1.0, x as f32 / 255.0]
                })
            })
            .collect();
        l.surface_mut().unwrap().write_region(Rect::new(0, 0, 256, 300), &px);
        doc.layers = vec![l];
        doc
    };
    for depth in [SampleType::U8, SampleType::U16] {
        let doc = doc_of(depth);
        let fmt = doc.pixel_format();
        let full = flatten_banded(&doc, fmt, SOURCE_BAND_PIXELS);
        let fast = plain_layer_surface(&mut doc.clone(), fmt).expect("a plain layer");
        assert_eq!(fast.content_bounds(), full.content_bounds(), "{depth:?}");
        assert_eq!(fast.default_bytes(), full.default_bytes(), "{depth:?}");
        let (f, g) = (fast.read_region(doc.bounds()), full.read_region(doc.bounds()));
        for (i, (p, q)) in f.as_chunks::<4>().0.iter().zip(g.as_chunks::<4>().0).enumerate() {
            assert_eq!(p, q, "{depth:?}: pixel {i}");
        }
    }
    // Whatever changes the composite, or the format, is flattened.
    let doc = doc_of(SampleType::U8);
    let fmt = doc.pixel_format();
    type Tweak = (&'static str, fn(&mut Document));
    let tweaks: [Tweak; 8] = [
        ("opacity", |d| d.layers[0].opacity = 0.5),
        ("blend", |d| d.layers[0].blend = BlendMode::Multiply),
        ("hidden", |d| d.layers[0].visible = false),
        ("mask", |d| d.layers[0].mask = Some(photocraft_doc::LayerMask::hide_all())),
        ("two layers", |d| {
            let l = d.layers[0].clone();
            d.layers.push(l);
        }),
        ("outside the canvas", |d| d.layers[0].surface_mut().unwrap().fill_rect(Rect::new(250, 290, 270, 310), &[1.0, 0.0, 0.0, 1.0])),
        ("an opaque default", |d| {
            let fmt = d.pixel_format();
            let mut s = Surface::with_default(fmt, &[1.0, 1.0, 1.0, 1.0]);
            s.fill_rect(Rect::new(0, 0, 10, 10), &[1.0, 0.0, 0.0, 1.0]);
            d.layers[0].content = LayerContent::Raster(s);
        }),
        ("multichannel", |d| d.mode = ColorMode::Multichannel),
    ];
    for (what, tweak) in tweaks {
        let mut d = doc.clone();
        tweak(&mut d);
        assert!(plain_layer_surface(&mut d, fmt).is_none(), "{what}");
    }
    let wide = PixelFormat::new(ColorMode::Rgb, SampleType::U16, true);
    assert!(plain_layer_surface(&mut doc.clone(), wide).is_none(), "another format");
}

#[test]
fn with_no_stage_budget_one_replace_places_each_placement_once() {
    // A render server keeps no stages between commands (smartStageCacheMb 0); within one Replace
    // Contents, copies at one place still share their placement, and a smart filter mask (which
    // mixes the placed and the filtered result) doesn't place the contents a second time.
    let _budget = stages::BudgetOverride::new(0);
    let dir = tmp_dir("place-once");
    let design = std::fs::read(png_file(&dir, "d.png", 90, 60, [0.2, 0.7, 0.4])).unwrap();
    let placed = || PLACEMENTS.with(std::cell::Cell::get);

    let (mut s, id) = smart_session();
    s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
    let before = placed();
    replace_contents_with(&mut s, id, "d.png", design.clone(), Fit::Stretch, true).unwrap();
    assert_eq!(placed() - before, 1, "two copies at one place");

    let (mut s, id) = smart_session();
    s.execute("filter.blur.gaussianBlur", json!({"layer": id.0, "radius": 2})).unwrap();
    s.edit("filter mask", |doc, _| {
        smart_mut(doc, id)?.filter_mask = Some(photocraft_doc::LayerMask::reveal_all());
        Ok(())
    })
    .unwrap();
    let before = placed();
    replace_contents_with(&mut s, id, "d.png", design, Fit::Stretch, true).unwrap();
    assert_eq!(placed() - before, 1, "a filter-masked smart object");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn with_no_source_budget_a_stretch_decodes_the_old_and_new_contents_once() {
    // A smart object with no stored size (no SoLd) takes its old box from its old contents. A
    // one-entry source cache must not trade them for the new file at every instance.
    let _cache = SOURCE_CACHE_TESTS.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _budget = SourceBudgetOverride::new(0);
    let dir = tmp_dir("decode-old-once");
    let (mut s, id) = smart_session();
    for _ in 0..2 {
        s.execute("layer.duplicate", json!({"layer": id.0})).unwrap();
    }
    let doc = &s.active().unwrap().doc;
    let copies: Vec<LayerId> = instances(doc, smart(doc, id).unwrap().contents_id).into_iter().filter(|&l| l != id).collect();
    for (k, copy) in copies.into_iter().enumerate() {
        set_placement(&mut s, copy, |sm| sm.transform.m[4] += 3.0 * (k + 1) as f64);
    }
    // A file no other test uses, so the shared source cache can't already hold it.
    let design = std::fs::read(png_file(&dir, "d.png", 97, 61, [0.13, 0.57, 0.91])).unwrap();
    let before = DECODES.with(std::cell::Cell::get);
    replace_contents_with(&mut s, id, "d.png", design, Fit::Stretch, true).unwrap();
    let decoded = DECODES.with(std::cell::Cell::get) - before;
    // The new file once, the old contents at most once (another test may have them cached).
    assert!((1..=2).contains(&decoded), "decoded {decoded} files for 3 instances");
    std::fs::remove_dir_all(&dir).ok();
}
