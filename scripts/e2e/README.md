# Real-workload simulator journeys

`fetch-real-workloads.py` downloads attributable public image, story and comic artifacts, normalizes the CBZ pages, and creates an ordinary notes folder and OPML subscription list. `run.py` sends each artifact through the released host command, asserts the host result, copies that prepared simulator state into a fresh simulator, then drives the corresponding app's committed route.

```sh
python3 scripts/e2e/fetch-real-workloads.py --out target/e2e-workloads
python3 scripts/e2e/run.py --workloads target/e2e-workloads --out target/e2e-results
```

Use repeated `--flow` arguments to run a subset. Outputs contain source hashes, host transcripts, simulator logs, screenshots and `results.json`. Nothing under `scripts/e2e` is a product fixture.

The committed app routes own interaction assertions, including solving the Nonograms board and playing the Parser story. TODO(OWNERQA): add case-specific route overrides here if an owner-journey item needs a path beyond the app's committed route.
