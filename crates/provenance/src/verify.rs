//! The c2pa SDK adapter: runs its reader and sorts its status codes into the three checks.

use std::io::Cursor;

use c2pa::validation_results::ValidationState;
use c2pa::{Context, Reader};

use crate::{Check, Error, Options, Report, State};

/// Hard-binding assertions: each proves the asset bytes are the ones that were signed.
const BINDING_PREFIXES: [&str; 4] = ["assertion.dataHash.", "assertion.boxesHash.", "assertion.bmffHash.", "assertion.collectionHash."];

pub(crate) fn read(bytes: &[u8], format: &str, options: &Options<'_>) -> Result<Option<Report>, Error> {
    let mut settings = serde_json::json!({
        // Only what is in the file is inspected: no remote manifests, no OCSP or TSA lookups.
        "verify": {"verify_trust": options.trust_anchors_pem.is_some(), "remote_manifest_fetch": false, "ocsp_fetch": false},
    });
    if let Some(pem) = options.trust_anchors_pem {
        settings["trust"] = serde_json::json!({"user_anchors": pem});
    }
    let context = Context::new().with_settings(settings).map_err(|e| Error::Malformed(format!("trust anchors: {e}")))?;
    let reader = match Reader::from_context(context).with_stream(format, Cursor::new(bytes)) {
        Ok(reader) => reader,
        // The label scan matched something that isn't a manifest store (e.g. text that says "c2pa").
        Err(c2pa::Error::JumbfNotFound) => return Ok(None),
        Err(e) => return Err(Error::Malformed(e.to_string())),
    };
    let Some(manifest) = reader.active_manifest() else {
        return Err(Error::Malformed("the manifest store has no active manifest".into()));
    };
    let codes = reader.validation_results().and_then(|r| r.active_manifest());
    let success: Vec<&str> = codes.map(|c| c.success().iter().map(|s| s.code()).collect()).unwrap_or_default();
    let failure: Vec<&str> = codes.map(|c| c.failure().iter().map(|s| s.code()).collect()).unwrap_or_default();

    // The claim signature covers the claim and, through hashed URIs, every assertion, so an
    // assertion changed after signing fails this check too. Trust and binding are reported apart.
    let breaks_signature = |c: &&&str| {
        let binding = BINDING_PREFIXES.iter().any(|p| c.starts_with(p)) || c.starts_with("claim.hardBindings.");
        let untrusted = *c == &"signingCredential.untrusted";
        !binding && !untrusted && ["claimSignature.", "signingCredential.", "assertion.", "claim."].iter().any(|p| c.starts_with(p))
    };
    let signature = match failure.iter().find(breaks_signature) {
        Some(code) => Check::Invalid((*code).to_string()),
        None if success.contains(&"claimSignature.validated") => Check::Valid,
        None => Check::Invalid("claimSignature.missing".into()),
    };
    let binding = if let Some(code) = failure.iter().find(|c| BINDING_PREFIXES.iter().any(|p| c.starts_with(p)) || c.starts_with("claim.hardBindings.")) {
        Check::Invalid((*code).to_string())
    } else if success.iter().any(|c| BINDING_PREFIXES.iter().any(|p| c.starts_with(p)) && c.ends_with(".match")) {
        Check::Valid
    } else {
        Check::Invalid("claim.hardBindings.missing".into())
    };
    let trust = if options.trust_anchors_pem.is_none() {
        Check::Unverified("no trust anchors configured".into())
    } else if success.contains(&"signingCredential.trusted") {
        Check::Valid
    } else {
        Check::Invalid(failure.iter().find(|c| c.starts_with("signingCredential.")).map_or("signingCredential.untrusted", |c| c).to_string())
    };
    let state = match reader.validation_state() {
        ValidationState::Trusted => State::Trusted,
        ValidationState::Valid => State::Valid,
        ValidationState::Invalid => State::Invalid,
    };
    let info = manifest.signature_info();
    Ok(Some(Report {
        // A failed check always wins over the SDK's summary, so the report never reads better
        // than its parts.
        state: if [&signature, &binding].iter().any(|c| matches!(c, Check::Invalid(_))) { State::Invalid } else { state },
        signature,
        binding,
        trust,
        claim_generator: manifest
            .claim_generator_info
            .as_ref()
            .and_then(|g| g.first())
            .map(|g| match &g.version {
                Some(v) => format!("{} {v}", g.name),
                None => g.name.clone(),
            })
            .or_else(|| manifest.claim_generator().map(str::to_string)),
        signer: info.and_then(|i| i.common_name.clone()),
        issuer: info.and_then(|i| i.issuer.clone()),
        signed_at: info.and_then(|i| i.time.clone()),
        title: manifest.title().map(str::to_string),
        failures: failure.iter().map(|c| (*c).to_string()).collect(),
    }))
}
