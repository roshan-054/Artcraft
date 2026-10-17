use photocraft_engine::Session;
use serde_json::json;

#[test]
fn recovery_blocks_plugin_entry_points_before_loading_code_and_preserves_editing() {
    let mut session = Session::new();
    session.plugins_disabled = true;
    session.execute("file.new", json!({"width":8,"height":6})).unwrap();
    for id in [
        "plugin.install",
        "plugin.reload",
        "plugin.run",
        "extension.inspect",
        "extension.install",
        "extension.invoke",
        "plugin.processImage",
        "plugin.runTask",
        "image.pluginPreview.apply",
    ] {
        assert!(!session.is_enabled(id), "{id}");
        let error = session.execute(id, json!({"path":"not-a-real-plugin", "id":"not-installed"})).unwrap_err().to_string();
        assert!(error.contains("disabled for this recovery session"), "{id}: {error}");
    }
    session.execute("layer.new.layer", json!({"name":"Recovery edit"})).unwrap();
    assert!(session.active().unwrap().doc.layers.iter().any(|layer| layer.name == "Recovery edit"));
    session.execute("edit.undo", json!({})).unwrap();
    session.execute("edit.redo", json!({})).unwrap();
    assert!(session.execute("extension.list", json!({})).is_ok());
    assert!(session.execute("extension.commands", json!({})).is_ok());
    assert!(!Session::new().plugins_disabled, "recovery policy must not survive another launch");
}

#[test]
fn recovery_preferences_keep_the_configured_filter_folder_without_loading_it() {
    let mut session = Session::new();
    session.plugins_disabled = true;
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("invert.wasm"), include_bytes!("../../plugins/tests/fixtures/invert.wasm")).unwrap();
    assert!(photocraft_plugins::registry::get("org.photocraft.example.invert").is_none());
    session.execute("prefs.set", json!({"path":"plugIns.useAdditionalPluginsFolder", "value":true})).unwrap();
    session.execute("prefs.set", json!({"path":"plugIns.additionalPluginsFolder", "value":folder.path().to_str().unwrap()})).unwrap();
    let prefs = session.prefs_to_json();
    assert!(!prefs.contains("pluginsDisabled"), "recovery policy must not be persisted");
    assert!(session.prefs().plug_ins.use_additional_plugins_folder);
    assert_eq!(session.prefs().plug_ins.additional_plugins_folder, folder.path().to_str().unwrap());
    assert!(photocraft_plugins::registry::get("org.photocraft.example.invert").is_none(), "the configured folder must not be scanned");
    assert!(session.plugins_disabled);
}
