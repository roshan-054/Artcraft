# Image Gain dependency notices

This independent starter and its copied Channel Gain filter are licensed under MIT OR
Apache-2.0; see LICENSE, LICENSE-MIT and LICENSE-APACHE. The filter is a copy of the neutral
PhotoCraft filter ABI starter, not a dependency on an installed plugin or the editor.

The panel and worker use serde_json and its locked transitive dependencies; the worker also
uses png and its locked dependencies. The build helper copies the exact dependency license
files for these locked builds into the package's `licenses/` directory from Cargo's local
registry source cache. Check those files and Cargo.lock when changing dependencies. Packaging
fails if the required dependency source or license files are missing. No model or external
runtime is required.
