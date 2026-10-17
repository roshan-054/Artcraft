use super::*;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "session.inspect",
            label: "Inspect Session",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: |s, _| Ok(inspect::session(s)),
            journal: false,
        },
        CommandSpec {
            id: "document.inspect",
            label: "Inspect Document",
            menu: &[],
            shortcut: None,
            params: r##"{"document":index?}"##,
            enabled: has_doc,
            run: |s, p| {
                let d = match p.get("document") {
                    Some(v) => {
                        let i = v.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or_else(|| bad("document.inspect", "`document` must be an index"))?;
                        s.documents().get(i).ok_or_else(|| EngineError::Other(format!("no document at index {i}")))?
                    }
                    None => s.active().ok_or(EngineError::NoDocument)?,
                };
                Ok(inspect::document(d))
            },
            journal: false,
        },
        CommandSpec {
            id: "document.activate",
            label: "Activate Document",
            menu: &[],
            shortcut: None,
            params: r##"{"document":index}"##,
            enabled: has_doc,
            run: |s, p| {
                let i = p.get("document").and_then(Value::as_u64).ok_or_else(|| bad("document.activate", "missing `document`"))?;
                let idx = usize::try_from(i).map_err(|_| bad("document.activate", "`document` out of range"))?;
                if s.set_active(idx) { Ok(Value::Null) } else { Err(EngineError::Other(format!("no document at index {idx}"))) }
            },
            journal: false,
        },
        CommandSpec {
            id: "document.move",
            label: "Move Document",
            menu: &[],
            shortcut: None,
            params: r##"{"document":index?,"to":index}"##,
            enabled: has_doc,
            run: |s, p| {
                let index = |key: &str| {
                    p.get(key)
                        .map(|v| v.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or_else(|| bad("document.move", format!("`{key}` must be a tab index"))))
                };
                let from = match index("document") {
                    Some(i) => i?,
                    None => s.active_index().ok_or(EngineError::NoDocument)?,
                };
                let to = index("to").ok_or_else(|| bad("document.move", "missing `to`"))??;
                let to = s.move_document(from, to).ok_or(EngineError::NoDocument)?;
                Ok(json!({"document": to}))
            },
            journal: false,
        },
        CommandSpec {
            id: "command.list",
            label: "List Commands",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: |s, _| Ok(Value::Array(command_specs().iter().map(|c| list_entry(s, c, (c.enabled)(s).err())).collect())),
            journal: false,
        },
        CommandSpec {
            id: "document.pixel",
            label: "Read Composite Pixel",
            menu: &[],
            shortcut: None,
            params: r##"{"x":i32,"y":i32}"##,
            enabled: has_doc,
            run: |s, p| {
                let coord = |k: &str| i32::try_from(int(p, k).unwrap_or(0)).map_err(|_| bad("document.pixel", format!("{k} must fit in 32 bits")));
                let (x, y) = (coord("x")?, coord("y")?);
                let d = s.active().ok_or(EngineError::NoDocument)?;
                // At i32::MAX the 1x1 rect saturates to empty: a pixel that far out is transparent.
                let px = photocraft_compose::render(&d.doc, Rect::from_xywh(x, y, 1, 1)).px.first().copied().unwrap_or_default();
                Ok(json!(px))
            },
            journal: false,
        },
    ]
}
