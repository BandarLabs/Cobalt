# Public test signing seed

`fixture-seed.hex` is the deliberately public, deterministic `[42; 32]` test
seed already used by `crates/kobo-cli/src/beta_store_smoke.rs` tests.
`tools/app-contribute.mjs --dry-run` expects it here to build local package and
catalog previews with `example.invalid` download URLs.

This is not a release credential. Never use it for production signing or add
its public key to a real reader's trusted Store keys. Official Beta publishing
uses a separate protected signing key; these previews cannot replace it.
