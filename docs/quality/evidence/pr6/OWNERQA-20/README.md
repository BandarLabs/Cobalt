# License and local-reference audit

The locked Cargo graph was read with `cargo metadata --locked`. Every registry/git package declares a license or license file. The root LICENSE, THIRD-PARTY.md and generated Rust dependency notice are present.

Every source file changed from `origin/beta` through the supplied integrated companion base `874ba0ee` was scanned for developer-home paths and copied/reference-implementation markers. None was found. Generated `file://` release URLs used by installer tests are allowed because they point at temporary test packages rather than reference source. This is a provenance hygiene check over shipped text and source; it does not claim authorship based only on textual similarity.
