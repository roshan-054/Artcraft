//! Linked smart objects keep their files' absolute paths, so moving or renaming a project folder,
//! or opening it on another machine, would leave every link missing (#6608). When a document is
//! opened from a file, a link whose file is gone is looked for below the document's own folder,
//! most specific first: `/old/project/art/logo.png`, linked from `/old/project/design.pcraft`, is
//! found as `<new folder>/art/logo.png`, else `<new folder>/logo.png`. Paths written on Windows
//! (`C:\Users\…\art\logo.png`) match the same way elsewhere, and the other way round.
//!
//! The search stays below the document's folder: no `..`, no drive, root or other prefix
//! component of the stored path is followed, so a document can't use it to reach other files
//! (#2927, #5609, #3837). Only paths are checked here; reading a found file still goes through
//! the usual linked-source reads and their authorization.

use std::path::{Path, PathBuf};

use photocraft_doc::{Document, LayerContent, LayerId, SmartSource};

/// Where the file of `link` may be now that its document lives in `dir`, most specific first:
/// `dir` joined with the last *k* components of `link`, longest first. Empty when `link` has a
/// `.` or `..` component.
fn candidates(dir: &Path, link: &str) -> Vec<PathBuf> {
    let parts: Vec<&str> = link.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.iter().any(|p| matches!(*p, "." | "..")) {
        return Vec::new();
    }
    // Nothing up to the last component with a `:` (a drive, or a URL-like prefix): on Windows
    // joining one would replace `dir` instead of extending it.
    let start = parts.iter().rposition(|p| p.contains(':')).map_or(0, |i| i + 1);
    (start..parts.len()).map(|i| parts[i..].iter().fold(dir.to_path_buf(), |d, p| d.join(p))).collect()
}

/// The links of `doc` (opened from `doc_path`) whose files are missing but were found below the
/// document's folder: (layer, the file found).
#[cfg(not(target_arch = "wasm32"))]
pub fn moved_links(doc: &Document, doc_path: &str) -> Vec<(LayerId, String)> {
    let Some(dir) = Path::new(doc_path).parent() else { return Vec::new() };
    let mut found = Vec::new();
    for (_, _, l) in doc.walk() {
        let LayerContent::Smart(sm) = &l.content else { continue };
        let SmartSource::Linked { path } = &sm.source else { continue };
        // Empty, still there, or a Photoshop link whose data the PSD itself holds (its uuid).
        if path.is_empty() || Path::new(path).exists() || photocraft_io::linked::find_linked_file(&doc.metadata, path).is_some() {
            continue;
        }
        if let Some(file) = candidates(dir, path).into_iter().find(|c| c.is_file())
            && let Some(file) = file.to_str()
        {
            found.push((l.id, file.to_string()));
        }
    }
    found
}

#[cfg(target_arch = "wasm32")]
pub fn moved_links(_: &Document, _: &str) -> Vec<(LayerId, String)> {
    Vec::new()
}

/// Point the links [`moved_links`] found at their files. Returns how many layers changed.
pub fn relink(doc: &mut Document, found: &[(LayerId, String)]) -> usize {
    let mut n = 0;
    for (id, file) in found {
        if let Some(l) = doc.layer_mut(*id)
            && let LayerContent::Smart(sm) = &mut l.content
            && let SmartSource::Linked { path } = &mut sm.source
        {
            *path = file.clone();
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::{Layer, SmartObject};
    use photocraft_geom::Affine;

    fn linked(doc: &mut Document, path: &str) -> LayerId {
        let l = Layer::new("link", LayerContent::Smart(SmartObject::new(SmartSource::Linked { path: path.into() }, Affine::IDENTITY, None)));
        doc.insert_above(None, l)
    }

    fn path_of(doc: &Document, id: LayerId) -> String {
        match &doc.layer(id).map(|l| &l.content) {
            Some(LayerContent::Smart(SmartObject { source: SmartSource::Linked { path }, .. })) => path.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn links_of_a_moved_project_are_found_below_its_folder() {
        let root = std::env::temp_dir().join(format!("pc-link-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("renamed project");
        std::fs::create_dir_all(project.join("art")).unwrap();
        std::fs::write(project.join("art").join("logo.png"), b"x").unwrap();
        std::fs::write(project.join("logo.png"), b"x").unwrap();
        std::fs::write(project.join("photo.jpg"), b"x").unwrap();
        std::fs::write(root.join("secret.png"), b"x").unwrap();
        let here = project.join("here.png");
        std::fs::write(&here, b"x").unwrap();
        let doc_path = project.join("design.pcraft");

        let mut doc = Document::new("design", photocraft_doc::Size::new(4, 4), photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8);
        let unix = linked(&mut doc, "/old/project/art/logo.png");
        let windows = linked(&mut doc, r"C:\Users\me\project\art\logo.png");
        let flat = linked(&mut doc, "/old/project/photos/photo.jpg");
        let present = linked(&mut doc, here.to_str().unwrap());
        let missing = linked(&mut doc, "/old/project/art/gone.png");
        let escape = linked(&mut doc, "/old/project/../secret.png");
        let uuid = linked(&mut doc, "");

        let found = moved_links(&doc, doc_path.to_str().unwrap());
        assert_eq!(relink(&mut doc, &found), 3, "{found:?}");
        let art = project.join("art").join("logo.png");
        assert_eq!(path_of(&doc, unix), art.to_str().unwrap(), "the most specific match wins over <folder>/logo.png");
        assert_eq!(path_of(&doc, windows), art.to_str().unwrap(), "a path written on Windows matches too");
        assert_eq!(path_of(&doc, flat), project.join("photo.jpg").to_str().unwrap(), "a file beside the document");
        assert_eq!(path_of(&doc, present), here.to_str().unwrap(), "a link whose file is there is left alone");
        assert_eq!(path_of(&doc, missing), "/old/project/art/gone.png", "nothing found: the link stays as it was");
        assert_eq!(path_of(&doc, escape), "/old/project/../secret.png", "`..` is never followed");
        assert_eq!(path_of(&doc, uuid), "");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// File › Open of a moved .pcraft (engine and CLI): its links point at the files again, and
    /// the document isn't marked modified.
    #[test]
    fn opening_a_moved_document_relinks_it() {
        let root = std::env::temp_dir().join(format!("pc-link-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("moved").join("art")).unwrap();
        let mut s = crate::Session::new();
        s.execute("file.new", serde_json::json!({"width": 4, "height": 4})).unwrap();
        let png = photocraft_io::export(&s.active().unwrap().doc, "logo.png", &Default::default()).unwrap().bytes;
        std::fs::write(root.join("moved").join("art").join("logo.png"), &png).unwrap();
        let mut doc = (*s.active().unwrap().doc).clone();
        let id = linked(&mut doc, "/old/place/art/logo.png");
        let bytes = photocraft_io::export(&doc, "design.pcraft", &Default::default()).unwrap().bytes;
        let doc_path = root.join("moved").join("design.pcraft");
        crate::file_cmds::open_bytes_as(&mut s, "design.pcraft", &bytes, None, Some(doc_path.to_str().unwrap().into())).unwrap();
        let st = s.active().unwrap();
        assert_eq!(path_of(&st.doc, id), root.join("moved").join("art").join("logo.png").to_str().unwrap());
        assert!(!st.is_dirty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn candidates_stay_below_the_folder() {
        let dir = Path::new("/docs");
        assert_eq!(candidates(dir, "/a/b/c.png"), [dir.join("a").join("b").join("c.png"), dir.join("b").join("c.png"), dir.join("c.png")]);
        assert_eq!(candidates(dir, r"D:\x\c.png"), [dir.join("x").join("c.png"), dir.join("c.png")]);
        assert_eq!(candidates(dir, r"\\server\share\c.png").last(), Some(&dir.join("c.png")));
        assert!(candidates(dir, "a/../c.png").is_empty());
        assert!(candidates(dir, "./c.png").is_empty());
    }
}
