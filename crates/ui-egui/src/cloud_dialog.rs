//! File › Open from the Cloud… and File › Save to the Cloud…: the shared cloud pages in a dialog.
//!
//! The Home screen carries the Open page in its Cloud section, but it is behind the tabs as soon as
//! a document is open, and Save As on the desktop is the operating system's own file panel, which
//! cannot hold a page of somebody's cloud files. So each of these is an ordinary dialog in the UI
//! state (`ui.inspect`, `ui.dialog.cancel` and the rest drive it) holding the page
//! `craft_ui::cloud` draws, the same one every Craft app offers.
//!
//! Neither page has an OK button. Opening a file and starting a save are what the pages' own
//! buttons do, and either one finishes the dialog. Nothing here is offered in a build with no
//! cloud service configured.

use egui::{Align, Layout, Ui};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;

/// The field that marks a dialog as this module's, holding which page it shows.
const MARK: &str = "__cloudPage";
const OPEN: &str = "open";
const SAVE: &str = "save";

/// The page a cloud dialog shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The user's own cloud files and what other people shared with them.
    Open,
    /// A folder to pick and a name to give the active document.
    Save,
}

/// Whether `fields` are one of this module's dialogs.
pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// The dialog's width: both pages are two columns, and a column needs room for a name and a size.
pub fn width(fields: &Map<String, Value>) -> Option<f32> {
    owns(fields).then_some(640.0)
}

/// Open the dialog on `page`, replacing any cloud dialog already up, and ask the service who is
/// signed in. Refused without a service, and a Save page with no document to save.
pub fn open(app: &mut PhotocraftApp, page: Page) -> Result<Value, String> {
    if !crate::cloud::available(app) {
        return Err(crate::cloud::UNAVAILABLE.to_owned());
    }
    if page == Page::Save && app.session.active().is_none() {
        return Err("no document open".to_owned());
    }
    app.ui.dialogs.retain(|d| !owns(&d.fields));
    let (mark, label) = match page {
        Page::Open => (OPEN, "Open from the Cloud…"),
        Page::Save => (SAVE, "Save to the Cloud…"),
    };
    let mut fields = Map::new();
    fields.insert(MARK.into(), json!(mark));
    fields.insert("__label".into(), json!(label));
    let id = app.ui.open_dialog(DialogKind::Command, fields);
    crate::cloud::start(app);
    Ok(json!({"dialog": id}))
}

/// The dialog's body. `Some(false)` closes it: the user cancelled, or the page started the work it
/// was opened for.
pub fn body(app: &mut PhotocraftApp, ui: &mut Ui, fields: &Map<String, Value>) -> Option<bool> {
    let page = fields.get(MARK).and_then(Value::as_str).unwrap_or(OPEN);
    let started = if crate::cloud::available(app) {
        if page == SAVE {
            // True once a save has started: the document is on its way, so the dialog is done.
            crate::cloud::save_page(app, ui)
        } else {
            crate::cloud::open_page(app, ui);
            matches!(app.cloud.busy, Some(crate::cloud::Busy::Opening { .. }))
        }
    } else {
        ui.label(crate::i18n::t(crate::cloud::UNAVAILABLE));
        false
    };
    if started {
        return Some(false);
    }
    ui.add_space(8.0);
    let mut cancel = false;
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        cancel = crate::widgets::secondary_button(ui, tl!("Cancel"), 84.0).clicked();
    });
    cancel.then_some(false)
}

/// Confirming a cloud dialog has nothing left to do: the pages act on their own buttons.
pub fn confirm() -> Result<Value, String> {
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> PhotocraftApp {
        PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default())
    }

    #[test]
    fn without_a_service_neither_page_opens() {
        let mut app = app();
        assert_eq!(open(&mut app, Page::Open), Err(crate::cloud::UNAVAILABLE.to_owned()));
        assert_eq!(open(&mut app, Page::Save), Err(crate::cloud::UNAVAILABLE.to_owned()));
        assert!(app.ui.dialogs.is_empty());
    }

    #[test]
    fn only_these_dialogs_are_wide_and_one_is_open_at_a_time() {
        let mut app = app();
        app.cloud.connect(Refusing);
        open(&mut app, Page::Open).unwrap();
        let fields = app.ui.dialogs.first().map(|d| d.fields.clone()).unwrap();
        assert!(owns(&fields));
        assert_eq!(width(&fields), Some(640.0));
        assert_eq!(width(&Map::new()), None);
        // A Save page needs a document, and replaces the Open page rather than stacking on it.
        assert_eq!(open(&mut app, Page::Save), Err("no document open".to_owned()));
        app.run("file.new", json!({"width": 8, "height": 8})).unwrap();
        open(&mut app, Page::Save).unwrap();
        assert_eq!(app.ui.dialogs.len(), 1);
        assert_eq!(app.ui.dialogs.first().and_then(|d| d.fields.get(MARK)).and_then(Value::as_str), Some(SAVE));
    }

    /// A backend that drops every job: enough to make the cloud available.
    struct Refusing;

    impl crate::cloud::CloudBackend for Refusing {
        fn submit(&self, _job: crate::cloud::Job, _post: crate::cloud::Post) {}
    }
}
