//! Opening a file records its C2PA Content Credentials (#1050), and a manifest that can't be read
//! never stops the open. The files signed at test time need the c2pa SDK:
//! `cargo test -p photocraft-io --features content-credentials-testgen --test content_credentials`.

use std::sync::Arc;

use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::provenance::{Check, Report, State};
use photocraft_doc::{Document, Layer, LayerContent};
use photocraft_geom::Size;
use photocraft_io::*;

fn one_layer(name: &str) -> Document {
    let mut d = Document::new(name, Size::new(4, 4), ColorMode::Rgb, SampleType::U8);
    d.layers.push(Layer::new("Layer 1", LayerContent::Raster(photocraft_raster::Surface::new(d.pixel_format()))));
    d
}

#[test]
fn a_file_without_credentials_records_none() {
    let png = export(&one_layer("plain"), "plain.png", &ExportOptions::default()).unwrap().bytes;
    let r = import("plain.png", &png).unwrap();
    assert_eq!(r.document.metadata.content_credentials, None);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn a_psd_cai_block_is_reported_as_present_but_not_checkable() {
    let mut d = one_layer("cai");
    d.metadata.psd_global_blocks.push((*b"8BIM", *b"CAI ", Arc::new(vec![7; 77])));
    let psd = export(&d, "cai.psd", &ExportOptions::default()).unwrap().bytes;
    let back = import("cai.psd", &psd).unwrap().document;
    let report = back.metadata.content_credentials.expect("the CAI block is reported");
    assert_eq!(report.state, State::Unsupported);
    assert_eq!(report.binding, Check::Unsupported("C2PA defines no container mapping for PSD files".into()));
    // The block itself still round-trips untouched.
    assert!(back.metadata.psd_global_blocks.iter().any(|(_, k, data)| k == b"CAI " && data.as_slice() == [7; 77]));
}

#[test]
fn a_psd_without_a_cai_block_records_none() {
    let psd = export(&one_layer("plain"), "plain.psd", &ExportOptions::default()).unwrap().bytes;
    assert_eq!(import("plain.psd", &psd).unwrap().document.metadata.content_credentials, None);
}

#[test]
fn credentials_are_not_saved_into_pcraft() {
    let mut opened = one_layer("opened");
    opened.metadata.content_credentials = Some(Report::unsupported("as opened"));
    let saved = export(&opened, "copy.pcraft", &ExportOptions::default()).unwrap().bytes;
    assert_eq!(import("copy.pcraft", &saved).unwrap().document.metadata.content_credentials, None);
}

#[cfg(feature = "content-credentials-testgen")]
mod signed {
    use photocraft_doc::provenance::testgen;

    use super::*;

    #[test]
    fn a_signed_file_records_its_checks_on_open() {
        let r = import("signed.png", &testgen::signed_png().unwrap()).unwrap();
        let report = r.document.metadata.content_credentials.expect("a manifest is present");
        assert_eq!(report.state, State::Valid);
        assert_eq!((&report.signature, &report.binding), (&Check::Valid, &Check::Valid));
        assert_eq!(report.trust, Check::Unverified("no trust anchors configured".into()));
        assert_eq!(report.signer.as_deref(), Some(testgen::SIGNER));
        assert_eq!(r.document.size, Size::new(4, 4), "the pixels open as usual");
    }

    #[test]
    fn a_damaged_manifest_still_opens_the_file_with_a_warning() {
        // Cut the manifest chunk short while keeping the PNG decodable: drop the tail of `caBX`.
        let signed = testgen::signed_png().unwrap();
        let start = signed.windows(4).position(|w| w == b"caBX").unwrap() - 4;
        let len = u32::from_be_bytes(signed[start..start + 4].try_into().unwrap()) as usize;
        let keep = len / 2;
        let mut damaged = signed[..start].to_vec();
        damaged.extend_from_slice(&(keep as u32).to_be_bytes());
        damaged.extend_from_slice(&signed[start + 4..start + 8 + keep]);
        damaged.extend_from_slice(&crc32fast::hash(&signed[start + 4..start + 8 + keep]).to_be_bytes());
        damaged.extend_from_slice(&signed[start + 12 + len..]);
        let r = import("damaged.png", &damaged).expect("the image still opens");
        let report = r.document.metadata.content_credentials.expect("the damaged manifest is reported");
        assert_eq!(report.state, State::Invalid);
        assert!(matches!(report.signature, Check::Invalid(_)), "{report:?}");
        assert!(r.warnings.iter().any(|w| w.starts_with("Content Credentials could not be read")), "{:?}", r.warnings);
    }
}
