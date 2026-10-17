//! Content Credentials inspection against files signed at test time by the `testgen` signer: the
//! acceptance criteria of #1050 for the inspect half.

use std::sync::OnceLock;

use photocraft_provenance::testgen::{self, ROOT_CA_PEM, UNRELATED_ROOT_CA_PEM};
use photocraft_provenance::{Check, Error, Options, Report, State, inspect};

fn no_anchors() -> Options<'static> {
    Options::default()
}

fn anchors(pem: &'static str) -> Options<'static> {
    Options { trust_anchors_pem: Some(pem) }
}

fn report(bytes: &[u8], options: &Options<'_>) -> Report {
    match inspect(bytes, options) {
        Ok(Some(r)) => r,
        other => panic!("expected a report, got {other:?}"),
    }
}

/// Signed once per test binary.
fn signed_png() -> &'static [u8] {
    static SIGNED: OnceLock<Vec<u8>> = OnceLock::new();
    SIGNED.get_or_init(|| testgen::signed_png().unwrap())
}

#[test]
fn signed_file_without_anchors_reports_signature_and_binding_but_not_trust() {
    let r = report(signed_png(), &no_anchors());
    assert_eq!(r.state, State::Valid);
    assert_eq!(r.signature, Check::Valid);
    assert_eq!(r.binding, Check::Valid);
    assert_eq!(r.trust, Check::Unverified("no trust anchors configured".into()));
    assert_eq!(r.claim_generator.as_deref(), Some(testgen::CLAIM_GENERATOR));
    assert_eq!(r.signer.as_deref(), Some(testgen::SIGNER));
    assert_eq!(r.issuer.as_deref(), Some("PhotoCraft tests"));
    assert_eq!(r.title.as_deref(), Some("red-4x4.png"));
    assert!(r.failures.is_empty(), "{:?}", r.failures);
}

#[test]
fn signed_file_with_its_root_as_anchor_is_trusted() {
    let r = report(signed_png(), &anchors(ROOT_CA_PEM));
    assert_eq!(r.state, State::Trusted);
    assert_eq!((&r.signature, &r.binding, &r.trust), (&Check::Valid, &Check::Valid, &Check::Valid));
}

#[test]
fn signer_outside_the_anchors_is_untrusted_but_still_intact() {
    let r = report(signed_png(), &anchors(UNRELATED_ROOT_CA_PEM));
    assert_eq!(r.signature, Check::Valid);
    assert_eq!(r.binding, Check::Valid);
    assert_eq!(r.trust, Check::Invalid("signingCredential.untrusted".into()));
    assert_ne!(r.state, State::Trusted);
}

#[test]
fn changed_pixels_break_the_binding_and_never_pass() {
    let mut tampered = signed_png().to_vec();
    let idat = tampered.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
    // A sample inside the stored deflate block: the image still decodes, with another colour.
    tampered[idat + 8] ^= 0xFF;
    for options in [no_anchors(), anchors(ROOT_CA_PEM)] {
        let r = report(&tampered, &options);
        assert_eq!(r.binding, Check::Invalid("assertion.dataHash.mismatch".into()));
        assert_eq!(r.state, State::Invalid);
        assert!(r.failures.contains(&"assertion.dataHash.mismatch".to_string()));
    }
}

#[test]
fn an_edited_assertion_fails_the_signature_check() {
    // The claim signs the hash of every assertion: changing one after signing must show.
    let mut edited = signed_png().to_vec();
    let at = edited.windows(14).position(|w| w == b"digitalCapture").unwrap();
    edited[at + 13] = b'f';
    let r = report(&edited, &anchors(ROOT_CA_PEM));
    assert_eq!(r.signature, Check::Invalid("assertion.hashedURI.mismatch".into()));
    assert_eq!(r.state, State::Invalid);
}

#[test]
fn corrupted_manifest_bytes_never_panic() {
    let signed = signed_png();
    let start = signed.windows(4).position(|w| w == b"caBX").unwrap();
    // Flip bytes across the manifest chunk. Some land in container framing the signature does
    // not cover (and still verify); the rest must come back as a clean error or a report.
    for at in (start..signed.len().min(start + 4096)).step_by(5) {
        let mut bad = signed.to_vec();
        if let Some(b) = bad.get_mut(at) {
            *b ^= 0x5A;
        }
        match inspect(&bad, &anchors(ROOT_CA_PEM)) {
            Ok(_) | Err(Error::Malformed(_)) => {}
            Err(e) => panic!("unexpected error at {at}: {e}"),
        }
    }
}

#[test]
fn truncated_files_return_errors_not_panics() {
    let signed = signed_png();
    for len in (8..signed.len()).step_by(97) {
        if let Ok(Some(r)) = inspect(&signed[..len], &no_anchors()) {
            assert_ne!(r.state, State::Valid, "truncated to {len} bytes still passed");
        }
    }
}

#[test]
fn report_serializes_with_one_result_per_check() {
    let v = serde_json::to_value(report(signed_png(), &no_anchors())).unwrap();
    assert_eq!(v["state"], "valid");
    assert_eq!(v["signature"], serde_json::json!({"result": "valid"}));
    assert_eq!(v["trust"], serde_json::json!({"result": "unverified", "reason": "no trust anchors configured"}));
    assert_eq!(v["signer"], testgen::SIGNER);
}

#[test]
fn file_without_a_manifest_reports_none() {
    assert_eq!(inspect(&testgen::plain_png(), &no_anchors()), Ok(None));
    assert_eq!(inspect(&[], &no_anchors()), Ok(None));
}

#[test]
fn the_word_c2pa_in_ordinary_metadata_is_not_a_manifest() {
    // A PNG tEXt chunk that merely mentions c2pa, inserted after IHDR.
    let plain = testgen::plain_png();
    let mut png = plain[..33].to_vec();
    let text = b"tEXtComment\0about c2pa";
    png.extend_from_slice(&((text.len() - 4) as u32).to_be_bytes());
    png.extend_from_slice(text);
    png.extend_from_slice(&[0, 0, 0, 0]);
    png.extend_from_slice(&plain[33..]);
    assert_eq!(inspect(&png, &no_anchors()), Ok(None));
}

#[test]
fn a_label_inside_an_unmapped_container_is_not_this_files_manifest() {
    // A PSD (or .pcraft) that embeds a signed image carries the label without a manifest of its own.
    let mut psd = b"8BPS\0\x01".to_vec();
    psd.extend_from_slice(&[0; 20]);
    psd.extend_from_slice(b"lnk2 embedded.jpg jumbjumdc2pa\0");
    assert_eq!(inspect(&psd, &no_anchors()), Ok(None));
}

#[test]
fn errors_become_reports_that_never_read_as_valid() {
    let r = Report::from_error(&Error::Malformed("box too short".into()));
    assert_eq!(r.state, State::Invalid);
    assert_eq!(r.signature, Check::Invalid("box too short".into()));
    let r = Report::from_error(&Error::TooLarge(usize::MAX));
    assert_eq!(r.state, State::Unsupported);
}
