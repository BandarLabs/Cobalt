# Real-workload simulator journeys

`fetch-real-workloads.py` downloads attributable public image, story and comic files, normalizes the CBZ pages, and writes an ordinary notes folder and an OPML subscription list. `run.py` builds the `kobo` CLI from this checkout, sends each workload through the matching host command (`vault`, `nonograms`, `parser`, `panels`, `feeds`) against the simulator, records the host output, copies the prepared simulator state into a fresh simulator, and drives the app with a short route defined in `run.py`.

```sh
python3 scripts/e2e/fetch-real-workloads.py --out target/e2e-workloads
python3 scripts/e2e/run.py --workloads target/e2e-workloads --out target/e2e-results
```

Repeat `--flow` (`vault`, `nonograms`, `parser`, `panels`, `rss`) to run a subset. Outputs contain source hashes, host transcripts, simulator logs, screenshots and `results.json`. Nothing under `scripts/e2e` is a product fixture.

The routes here check that the imported workload reaches the app: the note list and note in Vault, the imported photo puzzle in Nonograms, the story in the Parser library, the comic import and first page in Panels, and the feeds screen. Deeper interaction, such as solving a board or playing a story, belongs to each app's own committed route.
