//! Synthetic Content Credentials for tests (feature `testgen`; never in a release build).
//!
//! The certificates and key below are **test-only and public: never trust them outside tests.**
//! They were made with OpenSSL 3 (valid until 2126):
//!
//! ```sh
//! printf '[req]\ndistinguished_name=dn\nprompt=no\nx509_extensions=v3_ca\n[dn]\nCN=PhotoCraft Test Root CA\nO=PhotoCraft tests\n[v3_ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n' > ca.cnf
//! printf '[ext]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=emailProtection\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid\n' > ee.cnf
//! openssl genpkey -algorithm ed25519 -out ca.key
//! openssl req -x509 -new -key ca.key -config ca.cnf -days 36500 -out root.pem
//! openssl genpkey -algorithm ed25519 -out signer.key
//! openssl req -new -key signer.key -subj "/CN=PhotoCraft Test Signer/O=PhotoCraft tests" -out signer.csr
//! openssl x509 -req -in signer.csr -CA root.pem -CAkey ca.key -CAcreateserial -days 36500 -extfile ee.cnf -extensions ext -out signer.pem
//! openssl genpkey -algorithm ed25519 -out other.key
//! openssl req -x509 -new -key other.key -config ca.cnf -subj "/CN=Unrelated Test Root CA/O=PhotoCraft tests" -days 36500 -out unrelated.pem
//! ```

use std::io::Cursor;

use c2pa::{Builder, Context, SigningAlg, create_signer};

/// Self-signed Ed25519 root, `CN=PhotoCraft Test Root CA`: issued [`SIGNER_CERT_PEM`].
pub const ROOT_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBgDCCATKgAwIBAgIUBzC/vd+7ocZIaQKxkh/V3QA7MG0wBQYDK2VwMD0xIDAe
BgNVBAMMF1Bob3RvQ3JhZnQgVGVzdCBSb290IENBMRkwFwYDVQQKDBBQaG90b0Ny
YWZ0IHRlc3RzMCAXDTI2MTAxNzA4MjYyNVoYDzIxMjYwOTIzMDgyNjI1WjA9MSAw
HgYDVQQDDBdQaG90b0NyYWZ0IFRlc3QgUm9vdCBDQTEZMBcGA1UECgwQUGhvdG9D
cmFmdCB0ZXN0czAqMAUGAytlcAMhAKu9WdJsMT/sQgK6NoKxJYTe5vobhpymJ7yG
Xr1AXkw1o0IwQDAPBgNVHRMBAf8EBTADAQH/MA4GA1UdDwEB/wQEAwIBBjAdBgNV
HQ4EFgQU8FRoPZ0vvBgFCUkW7k4tL+BEIuMwBQYDK2VwA0EA+BWvCp7xpiRLvqjW
QKZ0Uc/Osiq8salkbw9a+rcY5cJfUWfbPPksIu4nMaja8XwjjrBPKKGRkPeexbAx
4YmJCw==
-----END CERTIFICATE-----
";

/// A second self-signed root that did not issue the signer (the untrusted-signer case).
pub const UNRELATED_ROOT_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBfjCCATCgAwIBAgIUAIwYFGWwa0VDXQyNxMxAZ9OlPHMwBQYDK2VwMDwxHzAd
BgNVBAMMFlVucmVsYXRlZCBUZXN0IFJvb3QgQ0ExGTAXBgNVBAoMEFBob3RvQ3Jh
ZnQgdGVzdHMwIBcNMjYxMDE3MDgzMDE0WhgPMjEyNjA5MjMwODMwMTRaMDwxHzAd
BgNVBAMMFlVucmVsYXRlZCBUZXN0IFJvb3QgQ0ExGTAXBgNVBAoMEFBob3RvQ3Jh
ZnQgdGVzdHMwKjAFBgMrZXADIQCf7fg0nvmsq98IwKW/8hYTOWpJi4qNXHQJ0jbW
JHzg0KNCMEAwDwYDVR0TAQH/BAUwAwEB/zAOBgNVHQ8BAf8EBAMCAQYwHQYDVR0O
BBYEFJjOsicufSrKXGzy5SvFFj0YP9/VMAUGAytlcANBANegtci3qk6Z3mXk3o0G
buVnSOaPnHlDvKS+WD/TJfQwaCMaTBDzZ7jLiTTmjLF+bw9LmwEsSZKZTEBKyC3Q
+Ac=
-----END CERTIFICATE-----
";

/// Ed25519 end-entity certificate (digitalSignature, emailProtection), `CN=PhotoCraft Test Signer`.
pub const SIGNER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBsjCCAWSgAwIBAgIUYCEf7srYuRhMt/rPx7wHFmsYK2YwBQYDK2VwMD0xIDAe
BgNVBAMMF1Bob3RvQ3JhZnQgVGVzdCBSb290IENBMRkwFwYDVQQKDBBQaG90b0Ny
YWZ0IHRlc3RzMCAXDTI2MTAxNzA4MjYyNVoYDzIxMjYwOTIzMDgyNjI1WjA8MR8w
HQYDVQQDDBZQaG90b0NyYWZ0IFRlc3QgU2lnbmVyMRkwFwYDVQQKDBBQaG90b0Ny
YWZ0IHRlc3RzMCowBQYDK2VwAyEA0iV7ZrLo2O+aDA94dVZzkOmqnhPO69cVpORw
npGr1fmjdTBzMAwGA1UdEwEB/wQCMAAwDgYDVR0PAQH/BAQDAgeAMBMGA1UdJQQM
MAoGCCsGAQUFBwMEMB0GA1UdDgQWBBQe6dHzzCqyNXpEzq9fAaGCp6hCgzAfBgNV
HSMEGDAWgBTwVGg9nS+8GAUJSRbuTi0v4EQi4zAFBgMrZXADQQCAUCcz5PkhzbZ/
mSf9hSVdDC65KC+Sun5EPi+l3YomgnbTNc7WOA6SXKQ3ASGo/6LsrkTfCKb2Ipcf
1NzYqXQO
-----END CERTIFICATE-----
";

/// The signer's private key. Test-only and public by design.
const SIGNER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIHB0KxgEE+TgX9ttZaCQ/RE7Dg4nNNMoi7SQZVapUNaG
-----END PRIVATE KEY-----
";

/// The claim generator the signed files name.
pub const CLAIM_GENERATOR: &str = "PhotoCraft tests 1.0";
/// The signer's subject common name.
pub const SIGNER: &str = "PhotoCraft Test Signer";

/// A 4×4 opaque red RGB PNG with no metadata (stored, uncompressed deflate).
pub fn plain_png() -> Vec<u8> {
    // Each row: filter byte 0 (none), then four red pixels.
    let row: Vec<u8> = std::iter::once(0).chain([200, 30, 30].repeat(4)).collect();
    let raw = row.repeat(4);
    let len = raw.len() as u16;
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 0, 0, 4, 0, 0, 0, 4, 8, 2, 0, 0, 0]);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// [`plain_png`] with a C2PA manifest (a `c2pa.created` action) signed by the test signer.
pub fn signed_png() -> Result<Vec<u8>, String> {
    let signer = create_signer::from_keys(SIGNER_CERT_PEM.as_bytes(), SIGNER_KEY_PEM.as_bytes(), SigningAlg::Ed25519, None).map_err(|e| e.to_string())?;
    let mut builder = Builder::from_context(Context::new())
        .with_definition(r#"{"claim_generator_info":[{"name":"PhotoCraft tests","version":"1.0"}],"title":"red-4x4.png"}"#)
        .map_err(|e| e.to_string())?;
    builder
        .add_action(serde_json::json!({"action": "c2pa.created", "digitalSourceType": "http://cv.iptc.org/newscodes/digitalsourcetype/digitalCapture"}))
        .map_err(|e| e.to_string())?;
    let mut out = Cursor::new(Vec::new());
    builder.sign(&signer, "image/png", &mut Cursor::new(plain_png()), &mut out).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = !0u32;
    for &b in kind.iter().chain(data) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}
