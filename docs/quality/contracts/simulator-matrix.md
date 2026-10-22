# Contract: simulator profile and text-scale matrix

Frozen by profile ID and measured firmware, not marketing name
(`crates/kobo-profile/src/lib.rs`, `SUPPORTED_PROFILES`).

## Profile matrix

| Class | Profile constant | Device code | Notes |
| --- | --- | --- | --- |
| compact monochrome | `CLARA_BW_391` | 391 | primary baseline, densest ordinary layout |
| compact monochrome variant | `CLARA_BW_395` | 395 | second Clara BW revision |
| older compact | `CLARA_HD_376` | 376 | older firmware/kernel, tighter performance |
| compact colour | `CLARA_COLOUR_393` | 393 | colour-to-monochrome semantic fallback |
| large monochrome | `ELIPSA_2E_389` | 389 | large panel, different touch distances |
| page-turn buttons | `LIBRA_2_388` | 388 | button mapping, landscape, asymmetric grip |
| large colour/buttons | `LIBRA_COLOUR_390` | 390 | largest interaction combination |
| large colour firmware variant | `LIBRA_COLOUR_390_446` | 390 (fw 4.46) | firmware-specific admission |
| regression protection | `LIBRA_H2O_384` | 384 | newly admitted hardware guard |

Every admitted pose of a profile is tested separately; launch-pose support
and live autorotation are different claims (issue #89).

## Text scales

All nine `TextScale::STEPS` (`crates/kobo-ui/src/lib.rs`): Smallest 80%,
Smaller 90%, Default 100%, Medium 110%, Large 120%, Larger 130%,
ExtraLarge 140%, Huge 155%, Largest 170%. Wire values 0-8 are written into
saved reading positions and never change. "Every supported text scale" in
acceptance criteria means all nine; smoke suites may run Default/Large/
ExtraLarge on each commit, the full nine before merge.

## Hermeticity

Each simulator runs with an explicit render environment - typesetter,
clock, profile, catalog, credentials - scoped to the run. No hidden mutable
process-global state; evidence and screenshots go to unique per-run
directories; dangling or cross-run symlinks are rejected. Selected suites
repeat 50 times in randomized parallel order before merge (issue #46).

## Tests

- Layout matrix: every reachable screen x affected profiles x nine scales.
- Screenshot baselines per profile; monochrome conversion of every
  colour-profile screenshot.
- Parallel-repeat gate on suites touching render state.
