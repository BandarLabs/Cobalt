# Readeck listing capture

The inbox image is a native Cobalt renderer snapshot at Clara BW dimensions,
using synthetic public sample articles. It is not a physical device capture or
an interactive simulator test. No account, token or network access is used.
The unmodified app receives a sample inbox response through `AppRunner`.

From the repository root:

```sh
python3 tools/review-captures/apps/rss-miniflux/screenshots/ui-review/capture.py \
  --root . --app readeck \
  --scenario tools/review-captures/apps/readeck/scenario.rs \
  --output target/readeck-listing
```

Copy `inbox.png` to `apps/readeck/screenshots/` and
`docs/media/site/apps/readeck/`, then run `node tools/generate-app-pages.mjs`.
The capture command also writes layout diagnostics and source/image hashes.
