# Describe Terminal persistence accurately

The launch screen claimed that nothing a shell does survives reboot. That contradicts the writable shell and can mislead users about file changes. The screen now says file changes can survive a reboot; launch behavior is unchanged.

## Evidence boundary

These PNGs are genuine **in-process app-renderer snapshots**, not interactive simulator screenshots or physical Kobo photographs. They use the original app screen builders, bundled typeface and `kobo-ui` renderer at Clara BW 1072 × 1448. The status strip is a deterministic fixture (00:00, 50% battery, connected radios). No pixels were redrawn or generated.

The interactive Cobalt simulator was built and its launch attempted, but this cloud environment rejects the Unix-domain socket bind with `Operation not permitted`, including the approved elevated launch. Interactive touch/network/refresh and physical hardware validation remain untested. Native callback/hit-test checks are stated separately.

Baseline is beta `9715304831eae95566758fd0aa6b8e6fc87ee3ee`. Each capture directory has source/provenance and layout diagnostics. The after source hash identifies the changed file; its recorded revision may be the parent commit because captures were made before committing.

## Before and after

### Terminal launch, 100% text

Before:

![Before: Terminal launch, 100% text](before/100-terminal-start.png)

After:

![After: Terminal launch, 100% text](after/100-terminal-start.png)

### Terminal launch, 170% text

Before:

![Before: Terminal launch, 170% text](before/170-terminal-start.png)

After:

![After: Terminal launch, 170% text](after/170-terminal-start.png)

## Validation

18 app tests; strict Clippy; formatting. Static ARM cross-build also passed. Launcher is bundled with the platform and has no Store app contribution manifest, so the per-app contributor dry-run does not apply.

## Reproduce renderer evidence

From the repository root with Python 3.11+ and Rust installed:

```sh
python3 examples/launcher/screenshots/ui-review/capture.py --out target/ui-review/launcher
```

To reproduce the original frame, run this same capture script with `--source-root` pointing to a separate checkout of the baseline commit. The helper selects the original or updated screen signature, adds only a temporary test probe, then deletes that probe. It does not start the app main function or modify app behavior.
