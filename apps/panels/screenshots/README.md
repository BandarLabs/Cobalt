# Panels screenshots

Actual SDK app in the Clara BW simulator, 1072 × 1448, extra-large interface,
original Cobalt artwork, ideal grayscale frame. These are not physical-device
captures. No live Komga service or account appears in them.

| Image | Source capture and provenance |
| --- | --- |
| library.png | [Cover and saved position restored](../../../docs/quality/evidence/panels-previews/37-shelf-position-restored.capture.json) |
| reader.png | [Reader after forced restart](../../../docs/quality/evidence/panels-recovery/44-recovered-after-restart.capture.json) |
| download-recovery.png | [Complete offline checkpoint](../../../docs/quality/evidence/panels-recovery/40-completed-download.capture.json) |
| import-guide.png | [USB folder step](../../../docs/quality/evidence/panels-docs/31-import-guide-folder.capture.json) |

The public app page uses the same library image at
`docs/media/site/apps/panels.png`. Capture metadata records the source checkout,
binary digest, fonts, profile, text scale, fixture and dirty-source status.

Reproduce with the built CLI and the same `CARGO_TARGET_DIR`:

```sh
python3 scripts/quality/check-comics-sim.py --output /tmp/panels-recovery --scale extra-large --download-recovery
python3 scripts/quality/check-comics-sim.py --output /tmp/panels-docs --scale extra-large --server-setup --onboarding --reader-tools --shelf-previews
```
