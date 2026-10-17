# Levels/Curves grayscale target follow-up

Completed 2026-10-17 against upstream main `2a952f164c0b4cf936b1c6497db10dd095341c00`. Follow-up to PR #5448 for issue #5335.

Levels now lists only the selected target by its actual name, using literal custom names and translated built-in names like Curves. Its controls and Auto use the matching parameter key and histogram for individual RGB/CMYK/Lab components. It uses grayscale controls and the edited plane's histogram for raster masks, alpha/spot channels and Quick Mask in every document colour mode. Curves/Levels Action dialogs resolve their recorded explicit target before choosing defaults. Their channel context survives Reset, pencil editing and sampling. Grayscale black/white eyedroppers sample the committed plane directly at native depth, including default samples, and update the visible neutral curve. Explicit scalar Action previews use the same CPU preview path as a normally selected scalar channel.

## Validation

- Full engine/UI `cargo test`: **4,988 passed, 0 failed, 32 normally ignored**, including doctest builds and all new tests.
- The ignored full-registry `panic_hunt` test passed separately.
- Strict engine/UI all-target Clippy (`-D warnings`), formatting, layering and `cargo xtask wasm` passed. Wasm reports three pre-existing cfg-specific unused-constant warnings (`PAR_PIXELS`, `MAX_SUBFOLDER_DEPTH`, `MAX_SUBFOLDER_FILES`); native strict Clippy is clean.
- `cargo xtask perf --quick` passed with `failures: []`. Quick mode uses small documents and does not apply full-size budgets. Its repository-baseline comparison was skipped because that baseline is from an M4 Pro; the separate comparison below uses the same machine for both builds.
- Full-app offscreen before/after screenshots were inspected for Lab Levels gamma, a Lab alpha white-point picker with a real canvas click, an alpha-targeted Action played while Red is selected, and Levels on a selected Green component. Updated candidate captures show `Saved sky` and `Green` in the Levels dropdown.
- Multiple local review/fix passes completed; the final review found no further actionable functional bugs within the reviewed scope.

The 18 new regression functions exercise RGB, CMYK, Lab and grayscale documents across native 8/16/32-bit samples; real Levels field editing; exact target names, literal spot names, translated built-in names, every RGB/CMYK/Lab component, Levels Auto histogram routing; black/white pickers on masks, alpha/spot and Quick Mask; serialized Action defaults and explicit target precedence; reset, pencil and sampling context; explicit pixel targets; preview/commit agreement; native-depth edge averages and unstored mask defaults; feathered selections; preservation of colour pixels, transparency and other planes; undo/redo, cancel and malformed inputs.

## 24 MP release comparison

Dense synthetic 6000 × 4000 RGB8 raster layer, raster mask, saved alpha channel and Quick Mask, each filled with 25% gray. The central selection is a 3000 × 2000 rectangle at 0.5 coverage. Proxy factor 4 (1500 × 1000). AMD Ryzen 7 9700X, 16 logical CPUs, Ubuntu 26.04.1, Rust 1.99.0, default optimized release profile (thin LTO).

Identical benchmark source at main `2a952f164c0b4cf936b1c6497db10dd095341c00` and the candidate. Three process runs per build, nine warmed samples per case per run: medians of 27 samples. Order: main/fix/fix/main/main/fix, with no competing compiler, test or screenshot workloads. Cold samples are retained separately and excluded. The RGB and grayscale values match in this fixture, so the white-point row produces equivalent edits in both builds.

Timing includes CPU proxy construction, command execution and flattening. It excludes histogram/dialog setup, GPU upload, UI drawing, worker scheduling and pointer-to-display latency. It measures the CPU fallback workload rather than the composite adjustment-layer GPU fast path.

| Target | Selection | Main (ms) | Fix (ms) | Change |
|---|---|---:|---:|---:|
| Composite | None | 39.92 | 37.35 | -6.4% |
| Composite | Central 50% coverage | 40.43 | 39.76 | -1.7% |
| Alpha | None | 75.95 | 72.82 | -4.1% |
| Alpha | Central 50% coverage | 41.95 | 40.38 | -3.7% |
| Raster mask | None | 71.89 | 70.36 | -2.1% |
| Raster mask | Central 50% coverage | 41.01 | 38.51 | -6.1% |
| Quick Mask | None | 73.89 | 73.42 | -0.6% |
| Quick Mask | Central 50% coverage | 41.74 | 40.48 | -3.0% |
| Alpha + white-point sample | None | 74.72 | 74.21 | -0.7% |
| Alpha + white-point sample | Central 50% coverage | 42.58 | 40.49 | -4.9% |

Timings were captured on the initial functional fix (`96dcb10f`); the later Levels dropdown naming/control update leaves this CPU benchmark workload unchanged.

No candidate median was slower in these cases. Small reductions are observations, not a claimed algorithmic speedup. Existing pixel-processing and proxy algorithms are reused; metadata resolution and a bounded 101 × 101 picker sample add no full-canvas scan to a drag frame. These measurements do not establish a 16 ms interactive budget.

Reproduce:

```sh
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo +1.99.0 run --release \
  -p photocraft-ui-egui --example tone_targets_bench -- \
  --reps 9 --out /tmp/tone-targets-bench.json
```

Raw samples, build logs, binary SHA-256 hashes, screenshot scripts and captures are retained locally in `plan/curves-followup-evidence/` (gitignored). The benchmark source and regression tests are included in the change.
