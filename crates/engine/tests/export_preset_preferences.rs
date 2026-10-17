use photocraft_engine::Session;
use serde_json::json;

#[test]
fn export_presets_survive_preferences_reload() {
    let mut session = Session::new();
    session.execute("file.export.presets.save", json!({"name": "Web", "settings": {"format": "png"}})).unwrap();
    let saved = session.prefs_to_json();
    let mut restored = Session::new();
    restored.load_prefs_json(&saved).unwrap();
    assert_eq!(restored.presets.export_presets, session.presets.export_presets);
    assert_eq!(restored.presets.export_presets.len(), 1);
}
