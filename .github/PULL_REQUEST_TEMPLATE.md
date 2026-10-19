<!-- Open this against the beta branch. main holds the stable release and
     changes reach it when a tested beta is promoted. -->

## What changes, and why

## Checks

- [ ] `cargo fmt --all --check`, `cargo test --workspace --all-targets --all-features`
      and `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass
- [ ] Tests cover the changed behaviour
- [ ] Any safety boundary this changes is named above
- [ ] For a change a reader would notice, a photo or short video of
      Settings ▸ About drawn on the device with the change installed

<!-- Contributing an app? Add ?template=app.md to this page's address to use
     the app template, and read docs/CONTRIBUTING_APPS.md. -->
