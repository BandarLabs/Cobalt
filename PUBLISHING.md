# Publishing the SDK crates

The public Rust API is split into small crates. Their local `path` dependencies
also carry exact `0.1.0` registry versions, so a workspace checkout uses local
source while a published crate resolves the same version from crates.io.

Publish a release in dependency order:

1. `kobo-ui` and `kobo-abi`
2. `kobo-protocol` and `kobo-text`
3. `kobo-policy`
4. `kobo-sdk`

For each layer, run `cargo publish --dry-run -p <crate>` and publish it before
checking the next layer. Cargo deliberately resolves registry dependencies when
packaging, so `kobo-sdk` cannot complete its dry run until the preceding crates
exist in the registry at the same version.

Runtime binaries, CLI tools and examples are distributed from this repository,
not as library crates. Tag releases from a clean commit and attach the generated
checksums and third-party notices with every binary package.
