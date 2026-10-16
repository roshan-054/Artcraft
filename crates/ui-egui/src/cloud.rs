//! PhotoCraft's half of Open and Save against cloud storage.
//!
//! The pages themselves are `craft_ui::cloud`: every Craft app offers the same ones. What is here
//! is what only PhotoCraft can answer. A document is an image document: the extensions and content
//! types below are what that means, and the app shell's own File › Open list
//! (`apps/photocraft/src/services.rs`, held to this one by a test there) is where they come from.
//! Bytes become a document through the same `open_bytes` File › Open uses, and go back through the
//! same exporter File › Save As uses (`Services::export`), so a cloud copy is exactly the file the
//! same save to disk would have written.
//!
//! PhotoCraft holds no provider credential and speaks no provider protocol. Every call goes to the
//! CollabCraft service, which is the single place a provider is configured and permissions are
//! decided. Replacing one storage provider with another changes nothing here.
//!
//! Nothing in this crate opens a socket either. The transport half is a [`CloudBackend`] the host
//! application supplies (see `apps/photocraft/src/cloud.rs`). Every service call blocks, and a
//! blocking read of a forty megabyte PSD inside a frame would freeze the window for seconds. So
//! the pages state a [`Job`], the host runs it on a worker thread, and the answer arrives on a
//! channel that [`pump`] drains on a later frame.

use std::collections::HashMap;

// Re-exported so a host application, and a test, can name these types without depending on the
// crates they come from.
pub use craft_cloud::{Capability, Entry, EntryKind};
pub use craft_ui::cloud::{
    Account, Busy, CloudApp, CloudBackend, CloudFiles, Conflict, CopyFormat, Destination, DocumentKind, Event, Fault, Folder, Job, Link, Origin, Post, Source,
    UNAVAILABLE, available, browse, can_converse, conflict_modal, destination_name, has_target, link, load_shared, open_entry, open_page, origin, parent_of,
    prefers_cloud, progress, pump, reload_active, save, save_as, save_page, share_button, shares, sign_in, sign_out, start, who, work_together,
};
use egui::{Color32, FontFamily, Painter, Rect, Ui};
use serde_json::{Value as Json, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;

/// The extensions a cloud file list offers to open: the documents and images File › Open turns
/// into a document, lower case and without a dot.
///
/// Left out on purpose, each because opening it does not end in a document this call can name:
/// camera raws (the Camera Raw dialog asks first), HEIC (a build option), and the preset files
/// (`.abr`, `.grd`, `.aco`, `.ase`, `.kys`), which go to their libraries, and `.psdt` templates,
/// which open as an untitled document with no file behind it. `apps/photocraft` holds this list
/// to a subset of File › Open's.
pub const OPEN_EXTS: &[&str] = &[
    "pcraft", "psd", "psb", "ora", "kra", "krz", "pdn", "af", "afdesign", "afphoto", "afpub", "png", "jpg", "jpeg", "tif", "tiff", "webp", "gif", "bmp", "tga",
    "ico", "qoi", "exr", "hdr", "pbm", "pgm", "ppm", "pam", "pfm", "pcx", "dds", "xpm", "fits", "fit", "fts", "dpx", "svg", "svgz",
];

/// Content types PhotoCraft opens, matched against the start of what the provider said.
///
/// Read beside the extension because neither is reliable alone. A WebDAV server hands back
/// `application/octet-stream` for a perfectly good `.psd`, and a file exported by a web
/// application often has a correct content type and no extension at all. Every entry has a row in
/// [`extension_for_mime`], and a test holds the two together.
const MIMES: [&str; 15] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/tiff",
    "image/bmp",
    "image/svg+xml",
    "image/vnd.adobe.photoshop",
    "image/x-psd",
    "image/x-tga",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/openraster",
    "image/x-exr",
    "image/vnd.radiance",
];

/// The file extension a content type stands for, for a destination whose name has none.
fn extension_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/tiff" => "tif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/vnd.adobe.photoshop" | "image/x-psd" => "psd",
        "image/x-tga" => "tga",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/openraster" => "ora",
        "image/x-exr" => "exr",
        "image/vnd.radiance" => "hdr",
        _ => return None,
    })
}

/// What a PhotoCraft document is, as a cloud file list needs to know.
///
/// A name typed with no known extension becomes `.pcraft`, the native format: it is the one that
/// keeps every layer, mask, effect and smart object, and a save the user did not choose a format
/// for should lose nothing. A PSD, an OpenRaster file or a flat image is a name away.
pub const DOCUMENT: DocumentKind = DocumentKind { extensions: OPEN_EXTS, mimes: &MIMES, noun: "document", default_extension: photocraft_format::EXTENSION };

/// The revision of each document a cloud save in flight encoded, by document id.
///
/// Only PhotoCraft knows what a version of a document is. The pages hold none of these. A save
/// that lands marks exactly the revision that reached the server as saved, so an edit made while
/// the bytes were on the wire stays unsaved.
pub type Sent = HashMap<u64, u64>;

/// Whether PhotoCraft can open this entry.
pub fn opens(entry: &Entry) -> bool {
    craft_ui::cloud::opens(entry, DOCUMENT)
}

/// A typed name as a document file name. Any folder separators are dropped and a missing
/// extension becomes `.pcraft`.
pub fn document_name(typed: &str) -> String {
    craft_ui::cloud::document_name(typed, DOCUMENT)
}

/// The whole of the cloud state, for the control channel and for tests.
pub fn status(app: &PhotocraftApp) -> Json {
    craft_ui::cloud::report(app)
}

/// Whether PhotoCraft writes a file with this lower-case extension: the layered formats and every
/// flat format the codecs can encode. The same set File › Save As offers.
fn writes(extension: &str) -> bool {
    photocraft_engine::file_cmds::layered_extension(extension) || photocraft_codecs::from_extension(extension).is_some_and(|f| f.caps().write)
}

/// The Save a Copy formats the Share menu offers under the share itself: none. PhotoCraft's Save
/// a Copy and Export As are dialogs with options of their own (quality, bit depth, metadata), and
/// a menu item that skipped them would write a file the user never got to choose the settings of.
pub fn copies() -> Vec<CopyFormat> {
    Vec::new()
}

/// Where the CollabCraft service is, as the environment names it.
///
/// Read from the environment and compiled in nowhere, because the service is deployment-specific
/// and a desktop binary must not carry one. The host application reads the same list, so the badge
/// names the deployment the worker actually reaches.
pub fn base_url() -> Option<String> {
    ["PHOTOCRAFT_CLOUD_URL", "CRAFT_CLOUD_URL", "COLLABCRAFT_URL"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .find(|value| !value.is_empty())
}

/// The host part of that address, for the account menu's first line.
///
/// Somebody with an account on two deployments needs to know which one a document is about to
/// land in.
fn service_host() -> String {
    base_url().map(|value| value.trim_start_matches("https://").trim_start_matches("http://").to_owned()).unwrap_or_default()
}

/// The file name a save should be exported under, for a destination the provider named.
///
/// The name is the first answer: its extension is what the file claims to be, and it is what the
/// user typed when they typed one. A provider that issued no extension at all still says what the
/// thing is, so the content type is the second. Both cases are common, and a file exported by a web
/// application usually has a correct content type and a bare name. The exporter picks the format
/// from the extension of what this returns.
///
/// A destination this application cannot write is refused by name, with the extensions it can
/// write. Encoding a document as `.pcraft` under the name `poster.kra` would hand somebody a file
/// nothing opens, and nothing would tell them. A refusal a person can act on is better.
fn export_name(into: Destination<'_>) -> Result<String, String> {
    let extension = into.extension();
    if !extension.is_empty() {
        return if writes(&extension) {
            Ok(into.name.to_owned())
        } else if OPEN_EXTS.contains(&extension.as_str()) {
            Err(cannot_write(&format!("PhotoCraft reads .{extension} files but cannot write one")))
        } else {
            Err(cannot_write(&format!("PhotoCraft does not write a .{extension}")))
        };
    }
    let mime = into.mime.ok_or_else(|| cannot_write("that name carries no extension"))?;
    let mime = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
    match extension_for_mime(&mime).filter(|e| writes(e)) {
        Some(extension) => Ok(format!("{}.{extension}", into.name)),
        None => Err(cannot_write(&format!("PhotoCraft does not write {mime}"))),
    }
}

/// A refusal that says what went wrong and which extensions are available instead.
fn cannot_write(why: &str) -> String {
    let writable: Vec<String> = OPEN_EXTS.iter().filter(|e| writes(e)).map(|e| format!(".{e}")).collect();
    format!("{why}. PhotoCraft writes {}", writable.join(", "))
}

impl PhotocraftApp {
    /// Keep a URL a control asked for while the window was being drawn ([`cloud`](self)).
    ///
    /// A paint call holds a shared borrow, and opening a URL wants a mutable one (it reaches the
    /// status bar and the platform service). So the URL waits one step rather than the opener
    /// being made reachable from everywhere.
    pub(crate) fn queue_url(&self, url: &str) {
        if let Ok(mut queue) = self.cloud_urls.try_borrow_mut() {
            queue.push(url.to_owned());
        }
    }

    /// Open what [`Self::queue_url`] kept. Once a frame, before anything is drawn.
    pub(crate) fn open_queued_urls(&mut self, ctx: &egui::Context) {
        let queued = self.cloud_urls.try_borrow_mut().map(|mut queue| std::mem::take(&mut *queue)).unwrap_or_default();
        for url in queued {
            crate::links::open(self, ctx, &url);
        }
    }
}

impl CloudApp for PhotocraftApp {
    fn cloud(&self) -> &CloudFiles {
        &self.cloud
    }

    fn cloud_mut(&mut self) -> &mut CloudFiles {
        &mut self.cloud
    }

    fn kind(&self) -> DocumentKind {
        DOCUMENT
    }

    fn active(&self) -> Option<u64> {
        self.session.active().map(|d| d.doc.id.0)
    }

    fn title(&self, uid: u64) -> String {
        self.session.documents().iter().find(|d| d.doc.id.0 == uid).map(|d| d.doc.name.clone()).unwrap_or_default()
    }

    fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        // The ordinary open path, with no file on this computer behind it: a cloud document holds
        // a provider handle instead of a path, and Save goes back through that handle.
        //
        // Read inline even where the app opens files as background jobs. The pages need the
        // document this opened to attach its origin to, and a background open has produced none
        // when this returns. The bytes are already in memory, so what is skipped is the decode
        // running off the UI thread, not a read.
        let before: Vec<u64> = self.session.documents().iter().map(|d| d.doc.id.0).collect();
        let background = std::mem::replace(&mut self.background_jobs, false);
        // Pass `&bytes` so Rust resolves to the inherent `open_bytes(&str, &[u8])` rather
        // than this trait method. Background jobs are suppressed so the document ID is
        // available before this call returns.
        let opened = self.open_bytes(name, &bytes);
        self.background_jobs = background;
        opened?;
        // A raw file or a colour profile prompt answers with a dialog rather than a document, and
        // a brush file goes to its library. Nothing is said about the bytes yet.
        Ok(self.session.documents().iter().map(|d| d.doc.id.0).find(|id| !before.contains(id)))
    }

    fn encode(&mut self, uid: u64, into: Destination<'_>) -> Result<Vec<u8>, String> {
        let name = export_name(into)?;
        let st = self.session.documents().iter().find(|d| d.doc.id.0 == uid).ok_or("no document open")?;
        let revision = st.revision;
        // The options the document was last saved with where it is going back out as what it
        // already is: a JPEG keeps its quality, a lossy WebP its setting.
        let settings = self.save_settings();
        let export = self.services.export.as_ref().ok_or("no exporter configured")?;
        let (bytes, warnings) = export(&st.doc, &name, &settings)?;
        // A format that cannot hold the document (a flat file for a layered one) is a copy, as in
        // File › Save As: the cloud file is written, and the document stays unsaved so the layers
        // are not forgotten.
        let copy = self.saves_copy(&name, None)?;
        if !copy {
            self.cloud_sent.insert(uid, revision);
        }
        crate::notices::io_warnings(self, &crate::i18n::fmt(crate::i18n::t("Saved {name}"), &[("name", &name)]), &warnings);
        Ok(bytes)
    }

    fn close(&mut self, uid: u64) {
        if let Some(index) = self.session.documents().iter().position(|d| d.doc.id.0 == uid) {
            // The command, not the session: it tears down what hangs off the document (views,
            // smart object windows) the way closing its tab does.
            let _ = self.run("file.close", json!({"document": index}));
        }
    }

    fn mark_saved(&mut self, uid: u64) {
        let Some(revision) = self.cloud_sent.remove(&uid) else { return };
        let _ = self.with_document(photocraft_doc::DocId(uid), |app| {
            if let Some(st) = app.session.active_mut() {
                st.saved_revision = revision;
            }
            Ok(())
        });
    }

    fn drop_pending(&mut self, uid: u64) {
        self.cloud_sent.remove(&uid);
    }

    fn ready_to_save(&mut self) -> bool {
        // Type being edited in place is not in the document yet. File › Save commits it first.
        if self.ui.text_edit.is_some() {
            crate::type_tool::commit(self);
        }
        true
    }

    fn sharing(&mut self) -> &mut craft_ui::share::ShareDialog {
        &mut self.share
    }

    fn presence(&mut self) -> &mut craft_ui::presence::PresenceBar {
        &mut self.presence
    }

    fn paint_icon(&self, painter: &Painter, rect: Rect, kind: EntryKind, colour: Color32) {
        let icon = if kind == EntryKind::Folder { "folder" } else { "file" };
        crate::icons::paint_with(painter, icon, rect, colour);
    }

    fn open_url(&self, url: &str) {
        // A shared borrow is all a paint call has. The URL waits in the queue the frame drains on
        // its way out ([`PhotocraftApp::open_queued_urls`]).
        self.queue_url(url);
    }

    fn say(&mut self, text: String) {
        // Something the user has to see: the notice a failed open already uses. The text arrives
        // translated, because the pages compose their sentences and pass them through `tr`.
        crate::notices::error(self, text);
    }

    fn toast(&mut self, text: String) {
        // The status bar, which is where this application says what just happened.
        self.ui.status = text;
        self.ui.status_error = false;
    }

    fn go_to_save_as(&mut self) {
        if let Err(e) = crate::cloud_dialog::open(self, crate::cloud_dialog::Page::Save) {
            self.ui.status = e;
            self.ui.status_error = true;
        }
    }

    fn app_name(&self) -> &str {
        "PhotoCraft"
    }

    fn tr(&self, text: &str) -> String {
        crate::i18n::t(text).to_owned()
    }

    fn service(&self) -> String {
        service_host()
    }

    fn save_note(&self) -> Option<String> {
        Some(self.tr("A name with no extension is saved as a PhotoCraft document (.pcraft)."))
    }

    fn style(&self, ui: &Ui) -> craft_ui::cloud::CloudStyle {
        let t = Tokens::get(ui.ctx());
        craft_ui::cloud::CloudStyle {
            surface: t.card,
            border: t.card_border,
            text: t.text,
            dim: t.text_dim,
            hover: t.hover,
            soft: t.accent_soft,
            family: FontFamily::Proportional,
            bold: FontFamily::Name("semibold".into()),
        }
    }
}

// ---------------------------------------------------------------- the shell's seams

/// The command ids this module answers, by the same names the control channel and the menus use.
const IDS: [&str; 9] =
    ["cloud.status", "cloud.open", "cloud.openFile", "cloud.browse", "cloud.shared", "cloud.link", "cloud.save", "cloud.saveAs", "cloud.reload"];

/// Whether `id` is a cloud command ([`menu`] runs it).
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Whether a cloud command is enabled: nothing without a service, and the ones that act on a
/// document need one open. `None` for an id that is not a cloud command.
pub fn is_enabled(app: &PhotocraftApp, id: &str) -> Option<bool> {
    let open = app.session.active().is_some() && app.services.export.is_some();
    Some(match id {
        "cloud.open" | "cloud.browse" | "cloud.shared" | "cloud.link" | "cloud.status" => available(app),
        "cloud.saveAs" | "cloud.save" | "cloud.reload" | "cloud.openFile" => available(app) && open,
        _ => return None,
    })
}

/// Run a cloud command, and File › Save for a document that came from the cloud, which writes
/// back to the cloud file it came from rather than asking for a file on this computer.
///
/// Every cloud command answers as soon as the job is stated, never when it is done: the transfer
/// runs on a worker thread, and `cloud.status` reports what is in flight. `None` for any other id.
pub(crate) fn menu(app: &mut PhotocraftApp, id: &str, params: &Json) -> Option<Result<Json, String>> {
    if id == "file.save" {
        return (params.get("path").is_none() && has_target(app)).then(|| save(app));
    }
    if !handles(id) {
        return None;
    }
    if id == "cloud.status" {
        return Some(Ok(status(app)));
    }
    if !available(app) {
        return Some(Err(UNAVAILABLE.to_owned()));
    }
    Some(match id {
        // The Home screen carries the Open page in its Cloud section, but with a document open
        // that screen is behind the tabs, so File › Open from the Cloud is a dialog holding the
        // same page.
        "cloud.open" => crate::cloud_dialog::open(app, crate::cloud_dialog::Page::Open),
        "cloud.browse" => {
            let path = params.get("path").and_then(Json::as_str).unwrap_or("");
            browse(app, path);
            Ok(json!({"folder": path}))
        }
        "cloud.shared" => {
            load_shared(app);
            Ok(Json::Null)
        }
        "cloud.link" => {
            link(app);
            Ok(status(app))
        }
        // Opens something already listed, by handle or by name. The listing is what the user can
        // see, which keeps a scripted open and a clicked one on the same footing.
        "cloud.openFile" => {
            let handle = params.get("id").and_then(Json::as_str);
            let name = params.get("name").and_then(Json::as_str).unwrap_or("");
            let found = app
                .cloud
                .mine
                .entries
                .iter()
                .chain(app.cloud.shared.entries.iter())
                .find(|e| handle.is_some_and(|handle| e.id == handle) || (!name.is_empty() && e.name == name))
                .cloned();
            match found {
                Some(entry) if opens(&entry) => {
                    open_entry(app, entry, None);
                    Ok(status(app))
                }
                Some(entry) => Err(format!("PhotoCraft cannot open {}", entry.name)),
                None => Err("no such file in the cloud listing".to_owned()),
            }
        }
        "cloud.save" => save(app),
        "cloud.saveAs" => match (params.get("folder").and_then(Json::as_str), params.get("name").and_then(Json::as_str)) {
            (Some(folder), Some(name)) => save_as(app, folder, name),
            _ => crate::cloud_dialog::open(app, crate::cloud_dialog::Page::Save),
        },
        "cloud.reload" => reload_active(app),
        _ => return None,
    })
}

// ---------------------------------------------------------------- where the pages sit

/// The Home screen's Cloud section: the user's own cloud files on one side and what other people
/// shared with them on the other, each with a menu holding Open, Share and Work together.
///
/// Drawn by `craft_ui::cloud`, so every Craft app offers the same page. Returns whether anything
/// was drawn: a build with no cloud service configured draws nothing.
pub fn home_section(app: &mut PhotocraftApp, ui: &mut Ui) -> bool {
    if !available(app) {
        return false;
    }
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(crate::i18n::t("Cloud")).font(crate::theme::semibold(12.5)).color(t.text_dim));
    ui.add_space(6.0);
    open_page(app, ui);
    true
}

/// Who is signed in and who else is in this document, then Share, at the title bar's far end where
/// every office suite puts them. Drawn first because the bar's controls run from the far end
/// inward, and what these take is subtracted from the room the rest divides up. Nothing is drawn
/// in a build with no cloud service.
pub fn title_controls(app: &mut PhotocraftApp, ui: &mut Ui) {
    if !available(app) {
        return;
    }
    who(app, ui);
    if shares(app) {
        ui.add_space(8.0);
        // A split button, drawn by craft-ui so every Craft app offers the same one.
        let _ = share_button(app, ui, &copies());
    }
}

/// Carry out a save the share dialog asked for, and report the outcome back.
///
/// Split into asking and answering because a cloud save is asynchronous. The dialog waits, showing
/// its own progress, until one of these arrives. A document that has never been in the cloud has no
/// handle to share, which is the one case this exists for.
pub(crate) fn share_save(app: &mut PhotocraftApp) {
    if let Some(request) = app.share.take_save_request() {
        // One destination today. `request.destination` names which, and is carried through so a
        // second provider needs no change here.
        let _ = request.destination;
        match save_as(app, "/", &request.name) {
            Ok(_) => app.cloud.sharing_save = true,
            Err(reason) => app.share.save_failed(reason),
        }
    }
    // The worker reports by clearing `busy`. A new origin means it landed.
    if app.cloud.sharing_save && app.cloud.busy.is_none() {
        app.cloud.sharing_save = false;
        match origin(app) {
            Some(origin) => {
                let (id, name) = (origin.entry.id.clone(), origin.entry.name.clone());
                app.share.saved(id, name);
            }
            None => {
                let reason = app.cloud.status.clone().unwrap_or_else(|| "the document was not saved".to_owned());
                app.share.save_failed(reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_content_type_is_one_the_extension_table_knows() {
        for mime in MIMES {
            let extension = extension_for_mime(mime);
            assert!(extension.is_some(), "{mime} has no extension");
            assert!(extension.is_some_and(|e| OPEN_EXTS.contains(&e)), "{mime} names an extension File › Open does not offer");
        }
        assert!(MIMES.contains(&"image/png"));
        assert!(!MIMES.contains(&"application/octet-stream"), "or every file in somebody's storage would be offered as a document");
        assert!(!MIMES.contains(&"application/json"), "or every JSON file would be offered as a document");
    }

    #[test]
    fn the_extensions_are_documents_and_images_not_presets_or_raws() {
        assert!(OPEN_EXTS.contains(&photocraft_format::EXTENSION), "the native format opens");
        assert!(OPEN_EXTS.contains(&"psd") && OPEN_EXTS.contains(&"png"));
        for left_out in ["abr", "grd", "aco", "ase", "kys", "psdt", "dng", "cr2", "nef", "heic"] {
            assert!(!OPEN_EXTS.contains(&left_out), ".{left_out} opens as something other than a document");
        }
        let mut seen = std::collections::HashSet::new();
        assert!(OPEN_EXTS.iter().all(|e| seen.insert(*e)), "one row per extension");
        assert!(OPEN_EXTS.iter().all(|e| *e == e.to_ascii_lowercase() && !e.starts_with('.')));
    }

    #[test]
    fn a_name_decides_the_format_and_a_bare_name_asks_the_content_type() {
        let name = |name: &str, mime: Option<&str>| export_name(Destination { name, mime });
        assert_eq!(name("Poster.pcraft", None), Ok("Poster.pcraft".to_owned()));
        assert_eq!(name("Poster.PSD", None), Ok("Poster.PSD".to_owned()), "an extension in any case");
        assert_eq!(name("Poster.png", None), Ok("Poster.png".to_owned()));
        // The name wins over the type, because the name is what somebody will double-click.
        assert_eq!(name("Poster.psd", Some("image/png")), Ok("Poster.psd".to_owned()));
        // No extension at all, and a type that says what the thing is.
        assert_eq!(name("exported", Some("image/png")), Ok("exported.png".to_owned()));
        assert_eq!(name("exported", Some("image/jpeg; charset=binary")), Ok("exported.jpg".to_owned()), "a type with a parameter on it");
        assert_eq!(name("exported", Some("image/vnd.adobe.photoshop")), Ok("exported.psd".to_owned()));
    }

    #[test]
    fn a_destination_this_app_cannot_write_is_refused_by_name() {
        let refused = |name: &str, mime: Option<&str>| export_name(Destination { name, mime }).err().unwrap_or_default();
        // Krita files are read and never written.
        let kra = refused("Art.kra", None);
        assert!(kra.contains("cannot write one"), "{kra}");
        assert!(kra.contains(".pcraft") && kra.contains(".psd"), "and it names what is available instead: {kra}");
        let unknown = refused("Art.wibble", None);
        assert!(unknown.contains(".wibble"), "{unknown}");
        // SVG is read, never written.
        assert!(refused("Art.svg", None).contains("cannot write one"));
        // A bare name with nothing to go on cannot be guessed at.
        assert!(refused("exported", None).contains("no extension"));
        let foreign = refused("exported", Some("application/octet-stream"));
        assert!(foreign.contains("application/octet-stream"), "{foreign}");
        // A readable type that is not writable is refused too.
        assert!(refused("exported", Some("image/svg+xml")).contains("image/svg+xml"));
    }

    #[test]
    fn a_typed_name_becomes_a_document_name() {
        assert_eq!(document_name(" Poster "), "Poster.pcraft");
        assert_eq!(document_name("Poster.psd"), "Poster.psd");
        assert_eq!(document_name("Poster.v2"), "Poster.v2.pcraft", "an unknown suffix is part of the name");
        assert_eq!(document_name("/a/b/Poster"), "Poster.pcraft", "a pasted path cannot become a folder");
        assert_eq!(document_name("   "), "");
    }

    #[test]
    fn the_share_menu_offers_no_save_a_copy_formats() {
        assert!(copies().is_empty());
    }

    #[test]
    fn a_build_with_no_service_offers_nothing_cloud_shaped() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        assert!(!available(&app));
        assert_eq!(is_enabled(&app, "cloud.open"), Some(false));
        assert_eq!(is_enabled(&app, "file.save"), None);
        for id in IDS.iter().filter(|id| **id != "cloud.status") {
            assert!(menu(&mut app, id, &json!({})).is_some_and(|r| r == Err(UNAVAILABLE.to_owned())), "{id} is refused");
        }
        assert!(menu(&mut app, "cloud.status", &json!({})).is_some_and(|r| r.is_ok()), "the state is always readable");
        assert!(menu(&mut app, "file.save", &json!({})).is_none(), "File › Save is the ordinary one");
        assert!(menu(&mut app, "edit.undo", &json!({})).is_none());
    }
}
