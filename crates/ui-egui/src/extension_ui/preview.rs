//! Host-owned before/after comparison. Only bounded display textures live in the shell;
//! the engine owns full-size results and validates every application to a document.
use std::sync::{Arc, Weak};

use egui::{Pos2, Rect, Vec2};
use photocraft_doc::Document;
use photocraft_engine::jobs::JobInfo;
use photocraft_geom::Rect as PixelRect;
use photocraft_plugins::extension::Manifest;
use serde_json::{Value, json};

use super::{PanelState, field};
use crate::{PhotocraftApp, theme::Tokens, widgets};

const THUMBNAIL_SIDE: u32 = 1536;
const LAYER_HINT: &str = "A same-size result can be applied as a new layer. Resized results need a new tab.";

#[derive(Clone)]
struct ImageCache {
    document: Weak<Document>,
    display: u64,
    crop: Option<PixelRect>,
    texture: egui::TextureHandle,
}

/// Ephemeral, never serialized. Cached per document snapshot, monitor transform and crop.
#[derive(Clone)]
pub(super) struct View {
    before: Option<ImageCache>,
    after: Option<ImageCache>,
    split: f32,
    actual_size: bool,
    center: Vec2,
    new_layer: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { before: None, after: None, split: 0.5, actual_size: false, center: egui::vec2(0.5, 0.5), new_layer: false }
    }
}

impl View {
    pub(super) fn clear_textures(&mut self) {
        self.before = None;
        self.after = None;
    }
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("split", &self.split)
            .field("actual_size", &self.actual_size)
            .field("center", &self.center)
            .field("new_layer", &self.new_layer)
            .finish()
    }
}

impl PartialEq for View {
    fn eq(&self, other: &Self) -> bool {
        self.split == other.split && self.actual_size == other.actual_size && self.center == other.center && self.new_layer == other.new_layer
    }
}

#[derive(Default)]
pub(super) struct Actions {
    pub event: Option<String>,
    pub cancel: Option<u64>,
    pub done: bool,
    pub discard: bool,
    pub apply: Option<&'static str>,
}

pub(super) fn discard(app: &mut PhotocraftApp, panel: &mut PanelState) {
    let completed = panel.job.take().and_then(|id| {
        app.session.cancel_job(photocraft_engine::jobs::JobId(id));
        app.session.jobs_with_recent().iter().find(|job| job.id.0 == id).and_then(|job| job.result.as_ref()?.get("preview")?.as_u64())
    });
    if let Some(id) = panel.preview.take().or(completed) {
        let _ = app.run("image.pluginPreview.discard", json!({"preview": id}));
    }
    panel.preview_view.clear_textures();
}

pub(super) fn validate(app: &mut PhotocraftApp, panel: &mut PanelState) {
    let Some(id) = panel.preview else { return };
    let problem = if panel.values != panel.preview_values {
        Some("Settings changed. Generate a new preview to apply them.".into())
    } else {
        app.session.validate_plugin_preview(id).err().map(|error| error.to_string())
    };
    if let Some(message) = problem {
        discard(app, panel);
        panel.error = false;
        panel.message = message;
    }
}

pub(super) fn apply(app: &mut PhotocraftApp, panel: &mut PanelState, target: &str) {
    let Some(id) = panel.preview else { return };
    if !crate::menus::modal_allows(app, "image.pluginPreview.apply") {
        panel.error = true;
        panel.message = "Finish or cancel the current dialog or crop before applying this preview.".into();
        return;
    }
    match app.run("image.pluginPreview.apply", json!({"preview": id, "target": target})) {
        Ok(result) => {
            panel.preview = None;
            panel.preview_view.after = None;
            panel.error = false;
            panel.message = result.get("message").and_then(Value::as_str).unwrap_or("Result applied.").into();
        }
        Err(error) => {
            panel.error = true;
            panel.message = error;
        }
    }
}

pub(super) fn window(app: &PhotocraftApp, ctx: &egui::Context, manifest: &Manifest, panel: &mut PanelState, job: Option<&JobInfo>) -> Actions {
    let mut action = Actions::default();
    let mut open = panel.open;
    let preview = panel.preview.and_then(|id| app.session.plugin_preview(id));
    let source = preview.map(|preview| preview.source.clone()).or_else(|| app.session.active().map(|state| state.doc.clone()));
    let result = preview.map(|preview| preview.result.clone());
    let layer_allowed = preview.is_some_and(|preview| preview.can_apply_as_layer());
    if !layer_allowed {
        panel.preview_view.new_layer = false;
    }
    let can_invoke = app.session.active_job().is_none() && crate::menus::modal_allows(app, "extension.invoke");
    let bounds = ctx.content_rect().shrink(8.0);
    let maximum = egui::vec2((bounds.width() - 24.0).max(240.0), (bounds.height() - 48.0).max(240.0));
    egui::Window::new(&manifest.name)
        .id(egui::Id::new(("extension-preview", &manifest.id)))
        .open(&mut open)
        .default_pos(egui::pos2(80.0, 75.0))
        .default_size(egui::vec2(1040.0, 680.0).min(maximum))
        .min_width(440.0_f32.min(maximum.x))
        .min_height(360.0_f32.min(maximum.y))
        .max_size(maximum)
        .constrain_to(bounds)
        .vscroll(false)
        .show(ctx, |ui| {
            let hint_height = if preview.is_some() && !layer_allowed {
                ui.painter().layout(LAYER_HINT.into(), egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color(), ui.available_width()).size().y
                    + ui.spacing().item_spacing.y
            } else {
                0.0
            };
            let height = (ui.available_height() - 66.0 - hint_height).max(1.0);
            if ui.available_width() >= 720.0 {
                // Bound the whole row, including its vertical separator. An unbounded
                // separator consumes the footer's height and grows the window each frame.
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.allocate_ui_with_layout(egui::vec2(270.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                        egui::ScrollArea::vertical().id_salt("plugin-settings").max_height(height).show(ui, |ui| {
                            settings(ui, manifest, panel, source.as_deref(), can_invoke, job, &mut action);
                        });
                    });
                    ui.separator();
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), egui::Layout::top_down(egui::Align::Min), |ui| {
                        comparison(app, ui, &mut panel.preview_view, source.as_ref(), result.as_ref(), height);
                    });
                });
            } else {
                egui::ScrollArea::vertical().id_salt("plugin-compact").max_height(height).show(ui, |ui| {
                    settings(ui, manifest, panel, source.as_deref(), can_invoke, job, &mut action);
                    ui.separator();
                    comparison(app, ui, &mut panel.preview_view, source.as_ref(), result.as_ref(), 280.0);
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("plugin-preview-output")
                    .width(130.0)
                    .selected_text(if panel.preview_view.new_layer { "New layer" } else { "New tab" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut panel.preview_view.new_layer, false, "New tab");
                        ui.add_enabled_ui(layer_allowed, |ui| {
                            ui.selectable_value(&mut panel.preview_view.new_layer, true, "New layer");
                        })
                        .response
                        .on_hover_text("New layers require a same-size result in an 8-bit or 16-bit RGB or grayscale document.");
                    })
                    .response
                    .on_hover_text("New layer is available for same-size results. Original layers are kept but hidden so transparency matches the preview. Resized results need a new tab.");
                let can_apply = panel.preview.is_some()
                    && panel.values == panel.preview_values
                    && job.is_none()
                    && crate::menus::modal_allows(app, "image.pluginPreview.apply");
                if ui.add_enabled_ui(can_apply, |ui| widgets::primary_button(ui, "Apply", 84.0)).inner.clicked() {
                    action.apply = Some(if panel.preview_view.new_layer { "layer" } else { "document" });
                }
                if ui.add_enabled(panel.preview.is_some(), egui::Button::new("Discard preview")).clicked() {
                    action.discard = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Done").clicked() {
                        action.done = true;
                    }
                });
            });
            if preview.is_some() && !layer_allowed {
                ui.weak(LAYER_HINT);
            }
        });
    panel.open = open;
    action
}

fn settings(
    ui: &mut egui::Ui,
    manifest: &Manifest,
    panel: &mut PanelState,
    source: Option<&Document>,
    can_invoke: bool,
    job: Option<&JobInfo>,
    action: &mut Actions,
) {
    ui.add(egui::Label::new(&manifest.description).wrap());
    ui.add_space(10.0);
    if let Some(source) = source {
        ui.add(egui::Label::new(egui::RichText::new(&source.name).strong()).wrap());
        ui.weak(format!("{} × {} pixels", source.size.width, source.size.height));
    } else {
        ui.weak("Open an image to generate a preview.");
    }
    ui.separator();
    for field_spec in &manifest.panel.fields {
        ui.push_id(&field_spec.id, |ui| field(ui, field_spec, &mut panel.values));
    }
    ui.add_space(10.0);
    if let Some(job) = job {
        ui.add(egui::ProgressBar::new(job.progress).show_percentage().text(&job.message));
        if ui.button("Cancel processing").clicked() {
            action.cancel = Some(job.id.0);
        }
    }
    for button in &manifest.panel.buttons {
        if ui.add_enabled_ui(can_invoke && job.is_none(), |ui| widgets::primary_button(ui, &button.label, 148.0)).inner.clicked() {
            action.event = Some(button.id.clone());
        }
    }
    if !panel.message.is_empty() {
        ui.add_space(10.0);
        ui.separator();
        ui.label(if panel.error { format!("Action failed: {}", panel.message) } else { panel.message.clone() });
    }
}

fn comparison(app: &PhotocraftApp, ui: &mut egui::Ui, view: &mut View, source: Option<&Arc<Document>>, result: Option<&Arc<Document>>, height: f32) {
    ui.horizontal(|ui| {
        ui.strong("Image preview");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.selectable_value(&mut view.actual_size, true, "100%");
            ui.selectable_value(&mut view.actual_size, false, "Fit");
        });
    });
    ui.weak(if view.actual_size { "Drag the image to pan. Drag the divider to compare." } else { "Drag the divider to compare the same area." });
    let result_height = if result.is_some() { ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y } else { 0.0 };
    let available = egui::vec2(ui.available_width().max(1.0), (height - 108.0 - result_height).max(1.0));
    let (stage, response) = ui.allocate_exact_size(available, egui::Sense::drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, ui.is_enabled(), "Image comparison"));
    let tokens = Tokens::get(ui.ctx());
    ui.painter().rect_filled(stage, 0.0, tokens.canvas);
    let Some(source) = source else {
        ui.painter().text(stage.center(), egui::Align2::CENTER_CENTER, "No image open", egui::FontId::proportional(14.0), tokens.text_dim);
        return;
    };
    let output = result.unwrap_or(source);
    let output_size = egui::vec2(output.size.width.max(1) as f32, output.size.height.max(1) as f32);
    let pixel_ratio = ui.ctx().pixels_per_point().max(f32::EPSILON);
    let crop_size = egui::vec2(
        (available.x * pixel_ratio).min(THUMBNAIL_SIDE as f32).min(output_size.x),
        (available.y * pixel_ratio).min(THUMBNAIL_SIDE as f32).min(output_size.y),
    );
    if view.actual_size && response.dragged() {
        let delta = ui.input(|input| input.pointer.delta());
        view.center -= delta * pixel_ratio / output_size;
    }
    view.center = view.center.clamp(Vec2::ZERO, Vec2::splat(1.0));
    let output_crop = view.actual_size.then(|| crop(output.size.width, output.size.height, view.center, crop_size));
    let source_crop = output_crop.map(|rect| mapped_crop(rect, output.size.width, output.size.height, source.size.width, source.size.height));
    let image_size =
        if view.actual_size { crop_size / pixel_ratio } else { output_size * (available.x / output_size.x).min(available.y / output_size.y).min(1.0) };
    let image_rect = Rect::from_center_size(stage.center(), image_size);
    let before = cached_image(app, ui.ctx(), &mut view.before, source, source_crop);
    let after = result.map(|result| cached_image(app, ui.ctx(), &mut view.after, result, output_crop));
    widgets::checker_rounded(ui.painter(), image_rect, 8.0, 0);
    let uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    let source_uv = output_crop.zip(source_crop).map_or(uv, |(output_crop, source_crop)| {
        mapped_uv(output_crop, source_crop, output.size.width, output.size.height, source.size.width, source.size.height)
    });
    // Texture pixels already contain the display transform; a neutral white tint preserves it.
    let neutral = egui::Color32::WHITE;
    if let Some(after) = after {
        let x = image_rect.left() + image_rect.width() * view.split;
        ui.painter()
            .with_clip_rect(image_rect.intersect(Rect::from_min_max(image_rect.min, egui::pos2(x, image_rect.bottom()))))
            .image(before, image_rect, source_uv, neutral);
        ui.painter()
            .with_clip_rect(image_rect.intersect(Rect::from_min_max(egui::pos2(x, image_rect.top()), image_rect.max)))
            .image(after, image_rect, uv, neutral);
        let handle = Rect::from_center_size(egui::pos2(x, image_rect.center().y), egui::vec2(18.0, image_rect.height()));
        let divider =
            ui.interact(handle, ui.id().with("comparison-divider"), egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if (divider.dragged() || divider.clicked())
            && let Some(pointer) = divider.interact_pointer_pos()
        {
            view.split = ((pointer.x - image_rect.left()) / image_rect.width().max(1.0)).clamp(0.0, 1.0);
            ui.ctx().request_repaint();
        }
        ui.painter().line_segment([egui::pos2(x, image_rect.top()), egui::pos2(x, image_rect.bottom())], egui::Stroke::new(2.0, tokens.text));
        ui.painter().rect_filled(Rect::from_center_size(egui::pos2(x, image_rect.center().y), egui::vec2(7.0, 32.0)), tokens.radius_sm, tokens.text);
    } else {
        ui.painter().image(before, image_rect, source_uv, neutral);
    }
    ui.horizontal(|ui| {
        ui.label("Original");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(if result.is_some() { "Processed" } else { "Generate a preview to compare" });
        });
    });
    ui.add_enabled_ui(result.is_some(), |ui| {
        ui.add(egui::Slider::new(&mut view.split, 0.0..=1.0).text("Comparison").show_value(false));
    });
    if let Some(result) = result {
        ui.weak(format!("Result · {} × {} pixels", result.size.width, result.size.height));
    }
}

fn crop(width: u32, height: u32, center: Vec2, size: Vec2) -> PixelRect {
    let w = (size.x.round() as u32).clamp(1, width.max(1));
    let h = (size.y.round() as u32).clamp(1, height.max(1));
    let x = (center.x * width as f32 - w as f32 * 0.5).round().clamp(0.0, width.saturating_sub(w) as f32) as i32;
    let y = (center.y * height as f32 - h as f32 * 0.5).round().clamp(0.0, height.saturating_sub(h) as f32) as i32;
    PixelRect::from_xywh(x, y, w, h)
}

fn mapped_crop(rect: PixelRect, from_w: u32, from_h: u32, to_w: u32, to_h: u32) -> PixelRect {
    let x = |value: i32| (f64::from(value) / f64::from(from_w.max(1))) * f64::from(to_w);
    let y = |value: i32| (f64::from(value) / f64::from(from_h.max(1))) * f64::from(to_h);
    PixelRect::new(x(rect.x0).floor() as i32, y(rect.y0).floor() as i32, x(rect.x1).ceil() as i32, y(rect.y1).ceil() as i32)
}

/// Keep a fractional source boundary aligned with the output's pixel crop, even at 3×/4×.
fn mapped_uv(output: PixelRect, source: PixelRect, from_w: u32, from_h: u32, to_w: u32, to_h: u32) -> Rect {
    let x = |value: i32| ((value as f32 / from_w.max(1) as f32) * to_w as f32 - source.x0 as f32) / source.width().max(1) as f32;
    let y = |value: i32| ((value as f32 / from_h.max(1) as f32) * to_h as f32 - source.y0 as f32) / source.height().max(1) as f32;
    Rect::from_min_max(egui::pos2(x(output.x0), y(output.y0)), egui::pos2(x(output.x1), y(output.y1)))
}

fn cached_image(
    app: &PhotocraftApp,
    ctx: &egui::Context,
    cache: &mut Option<ImageCache>,
    document: &Arc<Document>,
    crop: Option<PixelRect>,
) -> egui::TextureId {
    let (display, key) = crate::canvas::canvas_display(app, document, None);
    if let Some(cached) = cache
        && Weak::ptr_eq(&cached.document, &Arc::downgrade(document))
        && cached.display == key
        && cached.crop == crop
    {
        return cached.texture.id();
    }
    let buffer = match crop {
        Some(rect) => photocraft_compose::render_reduced_rect(document, rect, rect.width().min(THUMBNAIL_SIDE), rect.height().min(THUMBNAIL_SIDE)),
        None => photocraft_compose::thumbnail_buffer(document, THUMBNAIL_SIDE),
    };
    let image = crate::canvas::display_image(display.as_deref(), &buffer);
    let texture = ctx.load_texture("plugin-comparison", image, egui::TextureOptions::LINEAR);
    let id = texture.id();
    *cache = Some(ImageCache { document: Arc::downgrade(document), display: key, crop, texture });
    id
}

#[cfg(test)]
mod tests;
