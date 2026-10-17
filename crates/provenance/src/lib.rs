//! `photocraft-provenance`: reads and verifies C2PA Content Credentials (#1050).
//!
//! [`inspect`] looks at a file's bytes as they were opened, before any decoding, and reports what
//! an embedded C2PA manifest claims and which of its checks hold. The three checks are reported
//! separately, as the C2PA trust model keeps them apart:
//!
//! - **signature**: the claim signature verifies against the signing certificate;
//! - **binding**: the hard-binding hash matches the file's content over the ranges it covers;
//! - **trust**: the signing certificate chains to one of the trust anchors supplied.
//!
//! Nothing here is persisted or re-encoded: the report describes the file as opened, and editing
//! or re-saving a document neither updates it nor writes a manifest.
//!
//! The crate's API is plain data ([`Report`], [`Check`], [`Error`]) so the rest of PhotoCraft
//! knows nothing about the SDK. With the `c2pa` feature off, a file that carries a manifest is
//! reported as present with every check [`Check::Unsupported`].
//!
//! Never panics: every call into the SDK runs under `catch_unwind`, and a panic inside it becomes
//! [`Error::Malformed`].

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::Serialize;

#[cfg(feature = "testgen")]
pub mod testgen;
#[cfg(feature = "c2pa")]
mod verify;

/// Files larger than this are not inspected (the SDK hashes the whole asset).
pub const MAX_INPUT_BYTES: usize = 1 << 30;

/// The label every C2PA manifest store carries in its JUMBF description box. A file without it
/// cannot hold an embedded manifest, so most opens stop at this scan.
const MANIFEST_STORE_LABEL: &[u8] = b"c2pa";

/// One check's outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "result", content = "reason", rename_all = "camelCase")]
pub enum Check {
    Valid,
    /// The check ran and failed; the reason names the C2PA status code(s).
    Invalid(String),
    /// The check could not be decided with what was supplied (e.g. no trust anchors).
    Unverified(String),
    /// The check cannot run for this file or build (e.g. a container C2PA does not map).
    Unsupported(String),
}

/// The overall validation state, as the C2PA specification names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// Well-formed and cryptographically intact, and the signer chains to a trust anchor.
    Trusted,
    /// Well-formed and cryptographically intact; the signer was not checked against anchors.
    Valid,
    /// At least one check failed.
    Invalid,
    /// A manifest is present but this build or container cannot verify it.
    Unsupported,
}

/// What an embedded manifest claims and which checks hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub state: State,
    pub signature: Check,
    pub binding: Check,
    pub trust: Check,
    /// The claim generator (the application that signed), e.g. `"Adobe Photoshop 27.0"`.
    pub claim_generator: Option<String>,
    /// The signing certificate's subject common name.
    pub signer: Option<String>,
    /// The signing certificate's issuer organization.
    pub issuer: Option<String>,
    /// When the claim was signed (RFC 3339), if a time-stamp or signing time is present.
    pub signed_at: Option<String>,
    pub title: Option<String>,
    /// Every failure status code the validator reported, in order (`assertion.dataHash.mismatch`, …).
    pub failures: Vec<String>,
}

impl Report {
    /// A manifest is present but cannot be checked here (`reason` says why).
    pub fn unsupported(reason: &str) -> Self {
        Report {
            state: State::Unsupported,
            signature: Check::Unsupported(reason.into()),
            binding: Check::Unsupported(reason.into()),
            trust: Check::Unsupported(reason.into()),
            claim_generator: None,
            signer: None,
            issuer: None,
            signed_at: None,
            title: None,
            failures: Vec::new(),
        }
    }

    /// What to record when [`inspect`] fails: a manifest that can't be read is invalid (C2PA
    /// treats a malformed manifest as a failed validation), and a file too large to check is
    /// unsupported. The file itself still opens.
    pub fn from_error(error: &Error) -> Self {
        match error {
            Error::TooLarge(_) => Report::unsupported(&error.to_string()),
            Error::Malformed(why) => Report {
                state: State::Invalid,
                signature: Check::Invalid(why.clone()),
                binding: Check::Unverified("the manifest could not be read".into()),
                trust: Check::Unverified("the manifest could not be read".into()),
                ..Report::unsupported("")
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file is larger than [`MAX_INPUT_BYTES`].
    TooLarge(usize),
    /// A manifest is present but could not be read (truncated, corrupt, or the SDK panicked).
    Malformed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TooLarge(n) => write!(f, "file too large to check for Content Credentials ({n} bytes; the limit is {MAX_INPUT_BYTES})"),
            Error::Malformed(why) => write!(f, "Content Credentials could not be read: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Inspection options.
#[derive(Clone, Debug, Default)]
pub struct Options<'a> {
    /// PEM bundle of trusted root certificates. Without it the trust check is
    /// [`Check::Unverified`] and only the signature and binding are decided.
    pub trust_anchors_pem: Option<&'a str>,
}

/// The container C2PA defines for `bytes`, as a MIME type, from its signature.
pub fn sniff_format(bytes: &[u8]) -> Option<&'static str> {
    let at = |offset: usize, magic: &[u8]| bytes.get(offset..offset + magic.len()) == Some(magic);
    if at(0, &[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if at(0, b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("image/webp")
    } else if at(0, b"II*\0") || at(0, b"MM\0*") {
        Some("image/tiff")
    } else if at(0, b"GIF87a") || at(0, b"GIF89a") {
        Some("image/gif")
    } else if at(4, b"ftypavif") || at(4, b"ftypavis") {
        Some("image/avif")
    } else if at(4, b"ftypheic") || at(4, b"ftypheix") || at(4, b"ftypmif1") {
        Some("image/heif")
    } else if at(0, &[0xFF, 0x0A]) || at(4, b"JXL ") {
        Some("image/jxl")
    } else {
        None
    }
}

/// Whether `bytes` could hold an embedded manifest store (a cheap scan, no parsing).
pub fn may_carry_manifest(bytes: &[u8]) -> bool {
    memchr::memmem::find(bytes, MANIFEST_STORE_LABEL).is_some()
}

/// Reads and verifies the Content Credentials embedded in a file's bytes.
///
/// `Ok(None)`: the file carries no manifest, or is not a container C2PA maps (see
/// [`sniff_format`]; a PSD's `CAI ` block is the caller's to report, with
/// [`Report::unsupported`]). `Ok(Some(report))`: what the manifest claims and
/// which checks hold. `Err`: a manifest is there but could not be read, or the file is too large.
pub fn inspect(bytes: &[u8], options: &Options<'_>) -> Result<Option<Report>, Error> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge(bytes.len()));
    }
    if !may_carry_manifest(bytes) {
        return Ok(None);
    }
    // Outside the containers C2PA maps, the label can only come from something nested (a placed
    // image inside a PSD or `.pcraft`), which is not this file's manifest.
    let Some(format) = sniff_format(bytes) else {
        return Ok(None);
    };
    inspect_format(bytes, format, options)
}

#[cfg(feature = "c2pa")]
fn inspect_format(bytes: &[u8], format: &str, options: &Options<'_>) -> Result<Option<Report>, Error> {
    // The SDK is outside the workspace's never-panic lints; a panic there must not cost the user
    // the file they are opening.
    std::panic::catch_unwind(|| verify::read(bytes, format, options)).unwrap_or_else(|_| Err(Error::Malformed("the C2PA reader failed on this file".into())))
}

#[cfg(not(feature = "c2pa"))]
fn inspect_format(_: &[u8], _: &str, _: &Options<'_>) -> Result<Option<Report>, Error> {
    Ok(Some(Report::unsupported("this build of PhotoCraft cannot verify Content Credentials")))
}
