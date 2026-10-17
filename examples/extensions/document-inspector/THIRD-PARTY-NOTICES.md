# Third-party notices

This plugin is licensed under MIT OR Apache-2.0, as described in LICENSE.
It is built independently and does not link to PhotoCraft or another plugin.

The only direct library dependency is serde_json (MIT OR Apache-2.0).
Cargo.lock pins its transitive dependencies. The build copies the license files
for every resolved dependency into the package's `licenses/` directory, together
with a versioned dependency list. No externally installed runtime is required.

The ABI scaffold and packaging helper are copied from PhotoCraft's independently
licensed example starter, also MIT OR Apache-2.0; they have no host source dependency.
