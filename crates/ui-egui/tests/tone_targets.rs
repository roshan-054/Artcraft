//! Levels/Curves must use the same grayscale target in their controls, pickers and commands.
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{AlphaChannel, Document, Layer, LayerContent, LayerMask};
use photocraft_engine::{Session, channel_cmds::ChannelTarget};
use photocraft_geom::Size;
use photocraft_raster::Surface;
use photocraft_ui_egui::{PhotocraftApp, adjust_dialog, adjust_preview, dialogs, filter_dialog};
use serde_json::{Value, json};

const MODES: [ColorMode; 3] = [ColorMode::Rgb, ColorMode::Cmyk, ColorMode::Lab];
const GRAY_TARGET_MODES: [ColorMode; 4] = [ColorMode::Rgb, ColorMode::Cmyk, ColorMode::Lab, ColorMode::Grayscale];
const DEPTHS: [SampleType; 3] = [SampleType::U8, SampleType::U16, SampleType::F32];
const TARGETS: [ChannelTarget; 3] = [ChannelTarget::Alpha(0), ChannelTarget::LayerMask, ChannelTarget::Composite];

fn fixture(mode: ColorMode, depth: SampleType, target: ChannelTarget) -> PhotocraftApp {
    let mut doc = Document::new("Tone targets", Size::new(32, 32), mode, depth);
    let mut pixels = Surface::new(PixelFormat::new(mode, depth, true));
    pixels.fill_rect(
        doc.bounds(),
        &match mode {
            ColorMode::Cmyk => vec![0.1, 0.2, 0.3, 0.4, 1.0],
            ColorMode::Lab => vec![0.5, 0.5, 0.5, 1.0],
            ColorMode::Grayscale => vec![0.25, 1.0],
            _ => vec![0.0, 0.0, 0.0, 1.0],
        },
    );
    let mut gray = Surface::new(PixelFormat::new(ColorMode::Grayscale, depth, false));
    gray.fill_rect(doc.bounds(), &[0.25]);
    let mut layer = Layer::new("Named layer", LayerContent::Raster(pixels));
    layer.mask = Some(LayerMask { surface: gray.clone(), ..LayerMask::reveal_all() });
    let id = layer.id;
    doc.layers.push(layer);
    doc.channels.push(AlphaChannel::new("Saved sky", gray.clone()));
    if target == ChannelTarget::Composite {
        doc.quick_mask = Some(AlphaChannel::new("Quick Mask", gray));
    }
    let mut session = Session::new();
    session.add_document(doc, None);
    session.select_layer(id).unwrap();
    session.active_mut().unwrap().channel_view.target = target;
    let mut app = PhotocraftApp::new(session, Default::default());
    app.background_jobs = false;
    app
}

fn value(app: &PhotocraftApp, target: ChannelTarget) -> f32 {
    let doc = &app.session.active().unwrap().doc;
    match target {
        ChannelTarget::Alpha(i) => doc.channels[i].surface.sample_channel(8, 8, 0),
        ChannelTarget::LayerMask => doc.layers[0].mask.as_ref().unwrap().surface.sample_channel(8, 8, 0),
        ChannelTarget::Composite => doc.quick_mask.as_ref().unwrap().surface.sample_channel(8, 8, 0),
        _ => panic!("not a gray target"),
    }
}

fn harness(app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
    let h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_ui_state(|ui, app| dialogs::show(app, ui.ctx()), app);
    PhotocraftApp::setup_context(&h.ctx, Default::default());
    h
}

fn open(app: PhotocraftApp, kind: &str) -> Harness<'static, PhotocraftApp> {
    let mut h = harness(app);
    adjust_dialog::open(h.state_mut(), &format!("image.adjustments.{kind}")).unwrap();
    h.run_steps(4);
    h
}

fn params(h: &Harness<'static, PhotocraftApp>) -> Value {
    filter_dialog::params_of(&h.state().ui.dialogs.last().unwrap().fields)
}

fn names(h: &Harness<'static, PhotocraftApp>) -> Vec<String> {
    h.query_all_by_role(egui::accesskit::Role::ComboBox).filter_map(|n| n.value()).collect()
}

fn edit_gamma(h: &mut Harness<'static, PhotocraftApp>, gamma: &str) {
    h.query_all_by_role(egui::accesskit::Role::SpinButton).nth(1).unwrap().click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text(gamma.into()));
    h.run_steps(1);
    h.get_by_label("Preview").click();
    h.run_steps(2);
}

#[test]
fn levels_gray_target_controls_change_the_target_in_every_mode_and_depth() {
    for mode in GRAY_TARGET_MODES {
        for depth in DEPTHS {
            for target in TARGETS {
                let mut h = open(fixture(mode, depth, target), "levels");
                let name = match target {
                    ChannelTarget::Alpha(_) => "Saved sky",
                    ChannelTarget::LayerMask => "Named layer Mask",
                    _ => "Quick Mask",
                };
                assert_eq!(names(&h), [name], "{mode:?} {depth:?} {target:?}");
                // Type into the actual visible gamma field and blur it without confirming.
                edit_gamma(&mut h, "2.0");
                assert_eq!(params(&h)["gamma"], 2.0);
                let st = h.state().session.active().unwrap();
                let pixels = st.doc.layers[0].content.clone();
                let history = st.history.past_len();
                let before = value(h.state(), target);
                let id = h.state().ui.dialogs.last().unwrap().id;
                dialogs::confirm(h.state_mut(), id).unwrap();
                let after = value(h.state(), target);
                assert!(after > before + 0.1, "{mode:?} {depth:?} {target:?}: {before} -> {after}");
                assert_eq!(h.state().session.active().unwrap().doc.layers[0].content, pixels);
                assert_eq!(h.state().session.active().unwrap().history.past_len(), history + 1);
                h.state_mut().run("edit.undo", json!({})).unwrap();
                assert_eq!(value(h.state(), target), before);
                h.state_mut().run("edit.redo", json!({})).unwrap();
                assert_eq!(value(h.state(), target), after);
            }
        }
    }
}

#[test]
fn levels_isolates_each_colour_channel_and_edits_its_matching_record() {
    for mode in MODES {
        let labels: &[&str] = match mode {
            ColorMode::Rgb => &["Red", "Green", "Blue"],
            ColorMode::Cmyk => &["Cyan", "Magenta", "Yellow", "Black"],
            ColorMode::Lab => &["Lightness", "a", "b"],
            _ => unreachable!(),
        };
        let keys = photocraft_engine::adjust_params::channel_keys(photocraft_engine::adjust_params::default_space(mode));
        for depth in DEPTHS {
            for (k, label) in labels.iter().enumerate() {
                let mut app = fixture(mode, depth, ChannelTarget::Color(k));
                // Nonzero samples let gamma change each RGB component as well as Lab/CMYK.
                app.session
                    .edit("Colour pattern", |doc, _| {
                        let bounds = doc.bounds();
                        let n = mode.color_channels();
                        let mut values = vec![0.25; n + 1];
                        values[n] = 0.6;
                        doc.layers[0].surface_mut().unwrap().fill_rect(bounds, &values);
                        Ok(())
                    })
                    .unwrap();
                let mut h = open(app, "levels");
                assert_eq!(names(&h), [*label], "{mode:?} {depth:?} {k}");
                let before = params(&h);
                let original = h.state().session.active().unwrap().doc.clone();
                edit_gamma(&mut h, "2.0");
                let after = params(&h);
                assert_eq!(after[keys[k]]["gamma"], 2.0);
                for (i, key) in keys.iter().enumerate().filter(|(i, _)| *i != k) {
                    assert_eq!(after[key], before[key], "untargeted {i}");
                }
                let id = h.state().ui.dialogs.last().unwrap().id;
                dialogs::confirm(h.state_mut(), id).unwrap();
                let doc = &h.state().session.active().unwrap().doc;
                let old = original.layers[0].surface().unwrap().pixel(8, 8);
                let new = doc.layers[0].surface().unwrap().pixel(8, 8);
                assert!((new[k] - old[k]).abs() > 0.01, "{mode:?} {depth:?} {k}: {old:?} -> {new:?}");
                for i in (0..old.len()).filter(|i| *i != k) {
                    assert_eq!(new[i], old[i], "untargeted sample/transparency {i}");
                }
                assert_eq!(doc.channels, original.channels);
                assert_eq!(doc.layers[0].mask, original.layers[0].mask);
            }
        }
    }
}

#[test]
fn levels_uses_literal_spot_name_and_layers_panel_mask_name() {
    for mode in GRAY_TARGET_MODES {
        let mut app = fixture(mode, SampleType::U8, ChannelTarget::Alpha(0));
        app.session
            .edit("Named spot", |doc, _| {
                doc.channels[0].name = "Sky α — ink 1".into();
                doc.channels[0].spot = Some((photocraft_color::Color::rgb(0.2, 0.4, 0.6), 0.8));
                Ok(())
            })
            .unwrap();
        assert_eq!(names(&open(app, "levels")), ["Sky α — ink 1"]);

        let mut app = fixture(mode, SampleType::U8, ChannelTarget::LayerMask);
        app.session.active_mut().unwrap().channel_view.target = ChannelTarget::Composite;
        app.ui.mask_target = true;
        let mut h = open(app, "levels");
        assert_eq!(names(&h), ["Named layer Mask"]);
        assert_eq!(params(&h)["target"], "mask");
        let id = h.state().ui.dialogs.last().unwrap().id;
        let mut fields = h.state().ui.dialogs.last().unwrap().fields.clone();
        adjust_dialog::reset(h.state(), &h.ctx, &mut fields);
        h.state_mut().ui.dialog_mut(id).unwrap().fields = fields;
        h.run_steps(2);
        assert_eq!(names(&h), ["Named layer Mask"]);
        assert_eq!(params(&h)["target"], "mask");
    }
}

#[test]
fn levels_auto_uses_the_selected_colour_channels_histogram() {
    let mut app = fixture(ColorMode::Rgb, SampleType::U8, ChannelTarget::Color(1));
    app.session
        .edit("Distinct colour components", |doc, _| {
            let bounds = doc.bounds();
            doc.layers[0].mask = None;
            doc.layers[0].surface_mut().unwrap().fill_rect(bounds, &[0.1, 0.4, 0.8, 1.0]);
            Ok(())
        })
        .unwrap();
    let mut h = open(app, "levels");
    assert_eq!(names(&h), ["Green"]);
    let before = params(&h);
    h.get_by_label("Auto").click();
    h.run_steps(2);
    let after = params(&h);
    assert_eq!(after["green"]["inBlack"], 102.0);
    assert_eq!(after["green"]["inWhite"], 104.0);
    for key in ["red", "blue", "inBlack", "inWhite"] {
        assert_eq!(after[key], before[key]);
    }
}

#[test]
fn grayscale_picker_controls_sample_the_plane_and_change_the_visible_curve() {
    for kind in ["curves", "levels"] {
        for mode in GRAY_TARGET_MODES {
            for depth in DEPTHS {
                for target in TARGETS {
                    let mut h = open(fixture(mode, depth, target), kind);
                    assert!(h.get_by_label("Set Neutral Gray Point").accesskit_node().is_disabled());
                    assert!(!h.get_by_label("Set White Point").accesskit_node().is_disabled());
                    h.get_by_label("Set White Point").click();
                    h.run_steps(2);
                    assert!(adjust_dialog::picker_armed(h.state()));
                    let before = params(&h);
                    let original = h.state().session.active().unwrap().doc.clone();
                    assert!(adjust_dialog::sample_at(h.state_mut(), 8.0, 8.0));
                    let after = params(&h);
                    let key = if kind == "curves" { "points" } else { "inWhite" };
                    assert_ne!(before[key], after[key], "{kind} {mode:?} {depth:?} {target:?}");
                    let sampled_level = if kind == "curves" {
                        after["points"].as_array().unwrap().last().unwrap()[0].as_f64().unwrap()
                    } else {
                        after["inWhite"].as_f64().unwrap()
                    };
                    assert!((sampled_level - 63.75).abs() < 0.3, "sample the plane's 25% gray, not the colour layer: {after}");
                    for key in ["red", "green", "blue"] {
                        assert_eq!(before[key], after[key], "hidden {key} must not change");
                    }
                    assert!(std::sync::Arc::ptr_eq(&original, &h.state().session.active().unwrap().doc));
                    let id = h.state().ui.dialogs.last().unwrap().id;
                    dialogs::confirm(h.state_mut(), id).unwrap();
                    assert!(value(h.state(), target) > 0.99, "{kind} {mode:?} {depth:?} {target:?}");
                    assert_eq!(h.state().session.active().unwrap().doc.layers[0].content, original.layers[0].content);
                }
            }
        }
    }
}

fn action_dialog(mode: ColorMode, target: Value, panel: ChannelTarget, kind: &str) -> Harness<'static, PhotocraftApp> {
    let mut app = fixture(mode, SampleType::U8, panel);
    app.run("actions.record", json!({"name": "Tone step"})).unwrap();
    // Imported/serialized steps can carry document-space defaults alongside their explicit
    // grayscale target. Those records must not leak back into the dialog after normalization.
    let mut recorded = photocraft_engine::adjust_params::to_params(&photocraft_engine::adjust_params::default_for(kind, mode).unwrap());
    recorded["target"] = target;
    app.run(&format!("image.adjustments.{kind}"), recorded).unwrap();
    app.run("actions.stop", json!({})).unwrap();
    app.run("actions.dialog", json!({"action": "Tone step", "step": 0, "enabled": true})).unwrap();
    let mut h = harness(app);
    let result = h.state_mut().run("actions.play", json!({"action": "Tone step"})).unwrap();
    assert!(result.get("paused").is_some(), "{result:?}");
    h.run_steps(4);
    h
}

#[test]
fn action_dialog_uses_recorded_alpha_instead_of_current_red_channel() {
    for mode in MODES {
        for kind in ["curves", "levels"] {
            let mut h = action_dialog(mode, json!({"channel": 0}), ChannelTarget::Color(0), kind);
            assert_eq!(names(&h), ["Saved sky"]);
            assert_eq!(params(&h)["target"], json!({"channel": 0}));
            for key in ["cyan", "black", "lightness", "a", "b"] {
                assert!(params(&h).get(key).is_none());
            }
            let id = h.state().ui.dialogs.last().unwrap().id;
            let expected = if kind == "curves" {
                h.state_mut().ui.dialog_mut(id).unwrap().fields.insert("points".into(), json!([[0, 255], [255, 0]]));
                0.75
            } else {
                edit_gamma(&mut h, "2.0");
                0.5
            };
            let before = h.state().session.active().unwrap().doc.layers[0].content.clone();
            dialogs::confirm(h.state_mut(), id).unwrap();
            assert!((value(h.state(), ChannelTarget::Alpha(0)) - expected).abs() < 0.01);
            assert_eq!(h.state().session.active().unwrap().doc.layers[0].content, before);
        }
    }
}

#[test]
fn action_mask_target_survives_reset_pencil_and_picker() {
    for kind in ["curves", "levels"] {
        let mut h = action_dialog(ColorMode::Lab, json!("mask"), ChannelTarget::Alpha(0), kind);
        assert_eq!(names(&h), ["Named layer Mask"]);
        let id = h.state().ui.dialogs.last().unwrap().id;
        let mut fields = h.state().ui.dialogs.last().unwrap().fields.clone();
        adjust_dialog::reset(h.state(), &h.ctx, &mut fields);
        h.state_mut().ui.dialog_mut(id).unwrap().fields = fields;
        h.run_steps(2);
        assert_eq!(params(&h)["target"], "mask");
        if kind == "curves" {
            h.get_by_label("Draw to modify the curve").click();
            h.run_steps(2);
            assert_eq!(params(&h)["target"], "mask");
            assert_eq!(params(&h)["pencil"], true);
        }
        h.get_by_label("Set White Point").click();
        h.run_steps(2);
        assert!(adjust_dialog::sample_at(h.state_mut(), 8.0, 8.0));
        assert_eq!(params(&h)["target"], "mask");
        let alpha = h.state().session.active().unwrap().doc.channels[0].clone();
        dialogs::confirm(h.state_mut(), id).unwrap();
        assert!(value(h.state(), ChannelTarget::LayerMask) > 0.99);
        assert_eq!(h.state().session.active().unwrap().doc.channels[0], alpha);
    }
}

#[test]
fn explicit_channel_action_uses_channel_preview_even_with_composite_selected() {
    let mut h = action_dialog(ColorMode::Rgb, json!({"channel":0}), ChannelTarget::Color(0), "curves");
    h.state_mut().session.active_mut().unwrap().channel_view.target = ChannelTarget::Composite;
    assert!(!adjust_preview::on_layer(h.state_mut(), 0));
    assert_eq!(names(&h), ["Saved sky"]);
    let id = h.state().ui.dialogs.last().unwrap().id;
    h.state_mut().ui.dialog_mut(id).unwrap().fields.insert("points".into(), json!([[0, 255], [255, 0]]));
    let st = h.state().session.active().unwrap();
    let preview = filter_dialog::preview_document(&st.doc, st.active_layer, "image.adjustments.curves", &params(&h), 1).unwrap();
    dialogs::confirm(h.state_mut(), id).unwrap();
    assert_eq!(preview.channels, h.state().session.active().unwrap().doc.channels);
    assert_eq!(preview.layers, h.state().session.active().unwrap().doc.layers);
}

#[test]
fn explicit_pixels_action_bypasses_current_alpha_target() {
    let mut h = action_dialog(ColorMode::Rgb, json!("pixels"), ChannelTarget::Alpha(0), "curves");
    assert_eq!(names(&h), ["RGB"]);
    h.get_by_label("Set White Point").click();
    h.run_steps(2);
    assert!(adjust_dialog::sample_at(h.state_mut(), 8.0, 8.0));
    assert_eq!(params(&h)["target"], "pixels");
    let before = h.state().session.active().unwrap().doc.channels.clone();
    let id = h.state().ui.dialogs.last().unwrap().id;
    dialogs::confirm(h.state_mut(), id).unwrap();
    assert_eq!(h.state().session.active().unwrap().doc.channels, before);
}

#[test]
fn invalid_grayscale_sample_and_cancel_preserve_the_document() {
    let mut h = open(fixture(ColorMode::Lab, SampleType::F32, ChannelTarget::Alpha(0)), "curves");
    h.get_by_label("Set White Point").click();
    h.run_steps(2);
    let before = params(&h);
    let original = h.state().session.active().unwrap().doc.clone();
    for (x, y) in [(-1.0, 8.0), (99.0, 8.0), (f64::NAN, 8.0)] {
        assert!(adjust_dialog::sample_at(h.state_mut(), x, y));
        assert_eq!(params(&h), before);
    }
    let id = h.state().ui.dialogs.last().unwrap().id;
    // Automation can supply malformed private dialog fields too.
    h.state_mut().ui.dialog_mut(id).unwrap().fields.insert("__curvePickerBase".into(), json!([]));
    assert!(adjust_dialog::sample_at(h.state_mut(), 8.0, 8.0));
    assert_eq!(params(&h), before);
    dialogs::cancel(h.state_mut(), id).unwrap();
    assert!(std::sync::Arc::ptr_eq(&original, &h.state().session.active().unwrap().doc));
}

#[test]
fn resetting_an_armed_picker_does_not_restore_its_old_curve() {
    let mut h = open(fixture(ColorMode::Rgb, SampleType::U8, ChannelTarget::Alpha(0)), "curves");
    let id = h.state().ui.dialogs.last().unwrap().id;
    h.state_mut().ui.dialog_mut(id).unwrap().fields.insert("points".into(), json!([[0, 64], [255, 255]]));
    h.get_by_label("Set White Point").click();
    h.run_steps(2);
    assert!(adjust_dialog::sample_at(h.state_mut(), 8.0, 8.0));
    let mut fields = h.state().ui.dialogs.last().unwrap().fields.clone();
    adjust_dialog::reset(h.state(), &h.ctx, &mut fields);
    h.state_mut().ui.dialog_mut(id).unwrap().fields = fields;
    assert!(adjust_dialog::sample_at(h.state_mut(), 8.0, 8.0));
    assert_eq!(params(&h)["points"][0][1], 0.0);
}

#[test]
fn missing_explicit_channel_cannot_preview_as_a_colour_layer() {
    let mut h = open(fixture(ColorMode::Rgb, SampleType::U8, ChannelTarget::Color(0)), "curves");
    h.state_mut().session.active_mut().unwrap().channel_view.target = ChannelTarget::Composite;
    let id = h.state().ui.dialogs.last().unwrap().id;
    h.state_mut().ui.dialog_mut(id).unwrap().fields.insert("target".into(), json!({"channel":999}));
    assert!(!adjust_preview::on_layer(h.state_mut(), 0));
    let original = h.state().session.active().unwrap().doc.clone();
    assert!(dialogs::confirm(h.state_mut(), id).is_err());
    assert_eq!(h.state().session.active().unwrap().doc.channels, original.channels);
    assert_eq!(h.state().session.active().unwrap().doc.layers, original.layers);
}
