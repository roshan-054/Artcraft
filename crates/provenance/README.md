# photocraft-provenance

Reads and verifies C2PA Content Credentials embedded in image files (#1050). L0 standalone: no
workspace dependencies; `photocraft-doc` stores its [`Report`] and `photocraft-io` fills it on open.

```rust
use photocraft_provenance::{inspect, Options};

match inspect(&bytes, &Options::default())? {
    None => println!("no Content Credentials"),
    Some(r) => println!("{:?}: signature {:?}, binding {:?}, trust {:?}", r.state, r.signature, r.binding, r.trust),
}
```

- **Three checks, reported apart.** `signature` (the claim and its assertions are as signed),
  `binding` (the file's bytes match the signed hash) and `trust` (the signer chains to a supplied
  anchor; `unverified` without anchors). `state` is the C2PA validation state, and never reads
  better than a failed check.
- **Reads only what it is given.** No remote manifests, OCSP or time-stamp lookups.
- **Never panics.** Inputs over 1 GiB are refused, the SDK runs under `catch_unwind`, and a
  manifest that can't be read is `Error::Malformed`. `Report::from_error` turns that into an
  `invalid` report so the file still opens.
- **Containers.** JPEG, PNG, WebP, TIFF, GIF, AVIF/HEIF and JPEG XL, as the SDK maps them. Any
  other bytes report no manifest, so an image nested in a PSD or `.pcraft` is not mistaken for
  the file's own; `photocraft-io` reports a PSD's `CAI ` block as present but not checkable.

## Features

| Feature | Default | What |
|---|---|---|
| `c2pa` | no | Verification through the [c2pa](https://crates.io/crates/c2pa) SDK, pinned to `=0.90.22` (the last release with MSRV ≤ 1.95), pure-Rust crypto. Without it, a manifest is reported as present with every check `unsupported`. |
| `testgen` | no | `testgen::signed_png()`, `plain_png()` and test-only certificates; enables `c2pa`. `cargo test -p photocraft-provenance --features testgen`. |

`c2pa` is opt-in everywhere (`photocraft-io`'s `content-credentials`, forwarded by the engine and
both apps, like `heif`) because the SDK turns on `serde_json`'s `preserve_order`. Cargo unifies
features, so a build that includes it serializes every JSON object in insertion order instead of
sorted order.
