//! Cached payload sizes come from the engine's validated installation records.
use photocraft_engine::extension_cmds::StorageSize;

pub(super) fn bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        value /= 1024.0;
        unit = next;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

pub(super) fn label(storage: StorageSize) -> String {
    let installed = format!("Installed size: {}", bytes(storage.installed_bytes));
    if storage.model_bytes == 0 { installed } else { format!("{installed} · Models: {} (included)", bytes(storage.model_bytes)) }
}

pub(super) fn row(ui: &mut egui::Ui, storage: StorageSize) {
    ui.label(label(storage)).on_hover_text(format!(
        "{} bytes of plugin files, including {} bytes of models. Models are included in the installed total. This excludes temporary processing files and filesystem overhead.",
        storage.installed_bytes, storage.model_bytes
    ));
}
