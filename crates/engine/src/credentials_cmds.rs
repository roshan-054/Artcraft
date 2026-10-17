//! `document.contentCredentials` (#1050): the C2PA Content Credentials of the file a document
//! was opened from, checked on open by `photocraft-io` (`photocraft-provenance`).
//!
//! The result reports the signature, the asset binding and the signer's trust separately, plus
//! what the manifest claims (claim generator, signer, issuer, title). It describes the file as
//! opened: edits don't change it, and documents created in PhotoCraft or saved and reopened as
//! `.pcraft` report `none`. A query reading only in-memory state, so automation may call it.

use serde_json::{Value, json};

use crate::commands::{CommandSpec, has_doc};
use crate::{DocState, EngineError, Result, Session};

const CMD: &str = "document.contentCredentials";

/// The report for `d`, as `document.contentCredentials` and `document.inspect` show it.
pub fn credentials_json(d: &DocState) -> Value {
    match &d.doc.metadata.content_credentials {
        // Serializing plain strings and enums cannot fail; `none` would hide that, so say so.
        Some(report) => serde_json::to_value(report).unwrap_or_else(|e| json!({"state": "invalid", "error": e.to_string()})),
        None => json!({"state": "none"}),
    }
}

fn content_credentials(s: &mut Session, p: &Value) -> Result<Value> {
    let d = match p.get("document") {
        None | Some(Value::Null) => s.active().ok_or(EngineError::NoDocument)?,
        Some(v) => {
            let i = v
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| EngineError::BadParams { cmd: CMD.into(), msg: "`document` must be an index".into() })?;
            s.documents().get(i).ok_or_else(|| EngineError::Other(format!("no document at index {i}")))?
        }
    };
    Ok(credentials_json(d))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Content Credentials",
        menu: &[],
        shortcut: None,
        params: r##"{"document":index?}"##,
        enabled: has_doc,
        run: content_credentials,
        journal: false,
    }]
}

#[cfg(test)]
mod tests {
    use photocraft_doc::provenance::{Check, Report, State};

    use super::*;

    fn session_with(report: Option<Report>) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 4, "height": 4})).unwrap();
        let st = s.active_mut().unwrap();
        std::sync::Arc::make_mut(&mut st.doc).metadata.content_credentials = report;
        s
    }

    #[test]
    fn document_inspect_shows_the_same_report() {
        let mut s = session_with(Some(Report::unsupported("C2PA defines no container mapping for PSD files")));
        let v = s.execute(CMD, json!({})).unwrap();
        assert_eq!(v["state"], "unsupported");
        assert_eq!(s.execute("document.inspect", json!({})).unwrap()["contentCredentials"], v);
    }

    #[test]
    fn a_new_document_has_none() {
        let mut s = session_with(None);
        assert_eq!(s.execute(CMD, json!({})).unwrap(), json!({"state": "none"}));
    }

    #[test]
    fn reports_each_check_separately() {
        let report = Report {
            state: State::Valid,
            signature: Check::Valid,
            binding: Check::Invalid("assertion.dataHash.mismatch".into()),
            trust: Check::Unverified("no trust anchors configured".into()),
            claim_generator: Some("PhotoCraft tests 1.0".into()),
            ..Report::unsupported("")
        };
        let mut s = session_with(Some(report));
        let v = s.execute(CMD, json!({})).unwrap();
        assert_eq!(v["state"], "valid");
        assert_eq!(v["signature"], json!({"result": "valid"}));
        assert_eq!(v["binding"], json!({"result": "invalid", "reason": "assertion.dataHash.mismatch"}));
        assert_eq!(v["trust"], json!({"result": "unverified", "reason": "no trust anchors configured"}));
        assert_eq!(v["claimGenerator"], "PhotoCraft tests 1.0");
        assert_eq!(s.execute(CMD, json!({"document": 0})).unwrap(), v);
    }

    #[test]
    fn is_a_query_that_never_touches_history() {
        let mut s = session_with(Some(Report::unsupported("x")));
        let rev = s.active().unwrap().revision;
        s.execute(CMD, json!({})).unwrap();
        assert_eq!(s.active().unwrap().revision, rev);
        assert!(!s.active().unwrap().history.can_undo());
    }

    #[test]
    fn bad_params_are_errors_not_panics() {
        let mut s = Session::new();
        assert!(s.execute(CMD, json!({})).is_err(), "no document");
        assert!(!s.is_enabled(CMD));
        let mut s = session_with(None);
        for p in [
            json!({"document": 1}),
            json!({"document": -1}),
            json!({"document": "0"}),
            json!({"document": 1.5}),
            json!({"document": u64::MAX}),
            json!({"document": [0]}),
        ] {
            assert!(s.execute(CMD, p.clone()).is_err(), "{p}");
        }
    }
}
