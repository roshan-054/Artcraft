//! Plugins: one menu item per installed WebAssembly filter plug-in (`plugin.*` engine
//! commands), with a generated dialog and live preview for plug-ins that take parameters.

use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::menus::MenuItem;

/// Menu ids of installed plug-ins: `plugin.filter.<plug-in id>`.
const PREFIX: &str = "plugin.filter.";

/// Inserts the installed plug-ins into Plugins, above "Install Plug-in…".
pub fn insert_menu_items(app: &PhotocraftApp, items: &mut Vec<MenuItem>) {
    let plugins = photocraft_plugins::registry::list();
    let Some(at) = items.iter().position(|i| i.id == "plugin.install") else { return };
    // `MenuItem::path` is the untranslated English path from the command's engine spec: the menu
    // renderer groups rows by it and translates only at display time. Translating it here left the
    // plug-in rows under a path no menu node matched, so under any non-English language they were
    // never drawn (#2540). Borrow the path of "Install Plug-in…", the item we sit above.
    let Some(path) = items.get(at).map(|i| i.path.clone()) else { return };
    let enabled = app.session.is_enabled("plugin.run");
    let mut extra: Vec<MenuItem> = plugins
        .iter()
        .map(|p| {
            let m = p.manifest();
            let dots = if m.params.is_empty() || m.name.ends_with('…') { "" } else { "…" };
            MenuItem {
                id: format!("{PREFIX}{}", m.id),
                label: format!("{}{dots}", m.name),
                path: path.clone(),
                shortcut: None,
                enabled,
                checked: None,
                color: None,
            }
        })
        .collect();
    if !extra.is_empty() {
        extra.push(MenuItem { id: "---".into(), label: "---".into(), path: path.clone(), shortcut: None, enabled: false, checked: None, color: None });
    }
    for (k, it) in extra.into_iter().enumerate() {
        items.insert(at + k, it);
    }
}

/// Enabled state of a plug-in menu item (`None` for other ids).
pub fn is_enabled(app: &PhotocraftApp, id: &str) -> Option<bool> {
    let pid = id.strip_prefix(PREFIX)?;
    Some(photocraft_plugins::registry::get(pid).is_some() && app.session.is_enabled("plugin.run"))
}

/// Runs a plug-in menu item: straight away when it has no parameters (or params were given),
/// else through its dialog. "Install Plug-in…" without params opens the plugin manager.
pub fn menu(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let given = params.as_object().is_some_and(|o| !o.is_empty());
    if id == "plugin.install" && !given {
        return crate::extension_ui::menu(app, crate::extension_ui::MANAGER);
    }
    let pid = id.strip_prefix(PREFIX)?;
    let Some(plugin) = photocraft_plugins::registry::get(pid) else { return Some(Err(format!("no plug-in {pid:?} is installed"))) };
    let m = plugin.manifest();
    if m.params.is_empty() || given {
        let mut p = params.as_object().cloned().unwrap_or_default();
        p.insert("id".into(), json!(pid));
        return Some(app.run("plugin.run", Value::Object(p)));
    }
    let mut fixed = Map::new();
    fixed.insert("id".into(), json!(pid));
    let d = crate::filter_dialog::open_with_spec(app, "plugin.run", &m.name, &m.params_notation(), fixed);
    Some(Ok(json!({"dialog": d})))
}

/// Prefill the filter installation confirmation; only its existing OK action installs it.
pub(crate) fn review_filter(app: &mut PhotocraftApp, input: Value) -> Result<Value, String> {
    let mut fixed = input.as_object().cloned().ok_or("Select one .wasm filter file.")?;
    let has_path = fixed.get("path").and_then(Value::as_str).is_some_and(|path| !path.trim().is_empty());
    let has_data = fixed.get("data").and_then(Value::as_str).is_some_and(|data| !data.is_empty());
    if has_path == has_data {
        return Err("Select one .wasm filter file.".into());
    }
    fixed.insert("replace".into(), json!(false));
    // The encoded module remains a fixed command parameter rather than a potentially huge text
    // widget. A desktop path stays visible and editable in the normal confirmation dialog.
    let spec = if has_path { r#"{"path":text,"replace":bool=false}"# } else { r#"{"replace":bool=false}"# };
    let id = crate::filter_dialog::open_with_spec(app, "plugin.install", "Install Filter", spec, fixed);
    if let Some(dialog) = app.ui.dialog_mut(id) {
        // plugin.install changes the process-wide registry, so it must never run as a preview.
        dialog.fields.remove("__preview");
    }
    Ok(json!({"dialog": id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn module_data(id: &str) -> String {
        let manifest = json!({"id": id, "name": "Installation test filter", "version": "1", "kind": "filter"}).to_string();
        let escaped = manifest.replace('\\', "\\\\").replace('"', "\\\"");
        let bytes = wat::parse_str(format!(
            r#"(module
                (memory (export "memory") 1)
                (data (i32.const 16) "{escaped}")
                (func (export "pc_abi_version") (result i32) (i32.const 1))
                (func (export "pc_manifest") (result i64) (i64.const {packed}))
                (func (export "pc_alloc") (param i32) (result i32) (i32.const 0))
                (func (export "pc_filter") (param i32 i32 i32 i32 i32 i32 i32 i32)
                    (result i32) (i32.const 0)))"#,
            packed = ((manifest.len() as u64) << 32) | 16
        ))
        .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn explicit_install_parameters_keep_the_engine_installation_flow() {
        const ID: &str = "test.ui.explicit-install";
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let ctx = egui::Context::default();
        let result = crate::menus::invoke(&mut app, &ctx, "plugin.install", json!({"data": module_data(ID), "replace": false})).unwrap();
        assert_eq!(result["id"], ID);
        assert!(photocraft_plugins::registry::get(ID).is_some());
        assert!(!app.ui.extensions.manager_open);
        assert!(app.ui.dialogs.is_empty());
        photocraft_plugins::registry::remove(ID);
    }

    #[test]
    fn filter_review_prefills_path_without_reading_or_installing_it() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let path = "not-installed-yet.wasm";
        let result = review_filter(&mut app, json!({"path": path, "replace": true})).unwrap();
        let dialog = app.ui.dialogs.last().unwrap();
        assert_eq!(result["dialog"], dialog.id);
        assert_eq!(dialog.fields["__command"], "plugin.install");
        assert_eq!(dialog.fields["path"], path);
        assert_eq!(dialog.fields["replace"], false, "replacement requires an explicit choice in the review");
        assert!(!dialog.fields.contains_key("__preview"), "installation must never run as a live preview");
        assert!(!app.file_dialog_open());
    }

    #[test]
    fn filter_review_keeps_browser_data_inert_until_confirmed() {
        const ID: &str = "test.ui.review-install";
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let data = module_data(ID);
        let result = review_filter(&mut app, json!({"data": data})).unwrap();
        let id = result["dialog"].as_u64().unwrap();
        let dialog = app.ui.dialogs.last().unwrap();
        assert_eq!(dialog.fields["data"], data);
        assert_eq!(dialog.fields["replace"], false);
        assert!(!dialog.fields["__spec"].as_str().unwrap().contains("data"), "encoded bytes stay out of the text widgets");
        assert!(!dialog.fields.contains_key("__preview"));
        assert!(photocraft_plugins::registry::get(ID).is_none());
        crate::dialogs::confirm(&mut app, id).unwrap();
        assert!(photocraft_plugins::registry::get(ID).is_some());
        assert!(app.ui.dialogs.is_empty());
        photocraft_plugins::registry::remove(ID);
    }

    #[test]
    fn cancelling_filter_review_keeps_the_registry_unchanged() {
        const ID: &str = "test.ui.cancel-install";
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let result = review_filter(&mut app, json!({"data": module_data(ID)})).unwrap();
        crate::dialogs::cancel(&mut app, result["dialog"].as_u64().unwrap()).unwrap();
        assert!(photocraft_plugins::registry::get(ID).is_none());
        assert!(app.ui.dialogs.is_empty());
    }

    #[test]
    fn filter_review_rejects_missing_or_ambiguous_input() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        for input in [Value::Null, json!({}), json!({"path": ""}), json!({"path": "filter.wasm", "data": "bytes"})] {
            assert!(review_filter(&mut app, input).is_err());
        }
        assert!(app.ui.dialogs.is_empty());
    }

    #[test]
    fn installed_plugins_appear_under_filter_plugins_and_run() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let ctx = egui::Context::default();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins/tests/fixtures/invert.wasm");
        app.run("plugin.install", json!({"path": path})).unwrap();
        let items = crate::menus::menu_items(&app);
        let pi: Vec<&MenuItem> = items.iter().filter(|i| i.path == ["Plugins"]).collect();
        let me = pi.iter().find(|i| i.id == "plugin.filter.org.photocraft.example.invert").expect("plug-in listed");
        assert_eq!(me.label, "Invert (WebAssembly)");
        assert!(!me.enabled, "no document yet");
        assert!(pi.iter().any(|i| i.id == "plugin.install"), "Install Plug-in… is in the same submenu");
        app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
        assert!(crate::menus::is_enabled(&app, "plugin.filter.org.photocraft.example.invert"));
        let px = |app: &PhotocraftApp| app.session.active().unwrap().doc.layers[0].surface().unwrap().pixel(1, 1);
        let before = px(&app);
        crate::menus::invoke(&mut app, &ctx, "plugin.filter.org.photocraft.example.invert", json!({})).unwrap();
        assert!((px(&app)[0] - (1.0 - before[0])).abs() < 1e-6);
        assert!(crate::menus::invoke(&mut app, &ctx, "plugin.filter.not.installed", json!({})).is_err());
        // Install Plug-in… opens the shared manager rather than the legacy path dialog.
        let r = crate::menus::invoke(&mut app, &ctx, "plugin.install", json!({})).unwrap();
        assert_eq!(r, json!({"open": true}));
        assert!(app.ui.extensions.manager_open);
        assert!(app.ui.dialogs.is_empty());
    }

    /// Plug-in rows carry the untranslated English menu path like every other item, so they still
    /// land under Filter › Plug-ins when the UI language translates those menu names (#2540).
    #[test]
    fn plugin_menu_path_stays_untranslated_in_other_languages() {
        let ja = crate::i18n::Lang::from_code("ja").expect("registered language");
        // The names on the path must really differ from the English, or the test proves nothing.
        assert_ne!(crate::i18n::tr(ja, "Filter"), "Filter");
        assert_ne!(crate::i18n::tr(ja, "Plug-ins"), "Plug-ins");
        let _lang = crate::i18n::language_scope(ja);
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins/tests/fixtures/invert.wasm");
        app.run("plugin.install", json!({"path": path})).unwrap();
        let items = crate::menus::menu_items(&app);
        let me = items.iter().find(|i| i.id == "plugin.filter.org.photocraft.example.invert").expect("plug-in listed");
        assert_eq!(me.path, ["Plugins"], "plug-in row sits where the menu renderer looks for it");
        let install = items.iter().find(|i| i.id == "plugin.install").expect("Install Plug-in… listed");
        assert_eq!(me.path, install.path, "same submenu as Install Plug-in…");
    }
}
