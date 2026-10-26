# Contract: resource budgets (baseline)

Budgets are measured from current code, published, then tightened. A
regression above the agreed percentage requires an explanation or explicit
approval in the PR.

## What is measured (fixed fixtures, median and worst case)

- cold start and warm resume per app;
- peak RSS and steady-state memory;
- screen construction/layout time and time to first useful panel;
- local-search latency at small/medium/large libraries;
- full/partial refresh count for a standard journey;
- bytes downloaded and cache growth;
- background-job duration and wake frequency.

## Initial baseline

Measured in the implementation sandbox (2-core x86_64 host, simulator
profile `CLARA_BW_391`, rust 1.85.1 dev profile with `debug=0`):

| Metric | Value | Method |
| --- | --- | --- |
| simulator launch: Settings app to rendered frame, warm binary | ~21 s wall (includes runtime boot and app start on a 2-core host) | `kobo run --sim --app settings`, wall clock |
| kobo-cli dev build, full workspace deps | ~55 s | clean `cargo build -p kobo-cli` |
| workspace unit tests | 90 crates, all green except 3 kobo-cli ARM-gcc tests (environmental) | per-crate `cargo test` sweep |

These are starting numbers, not promises: the simulator shares the
renderer, layout engine and refresh planner with the device, but panel,
radio and power behavior need hardware evidence. Regression percentage is
unset until nightly measurement runs exist; the CI lane records values
before any budget is enforced.

## Tests

- Performance lane records the metrics above per run and stores history.
- Screenshot/evidence output goes to unique directories per run.
