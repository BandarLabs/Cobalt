# Device census: all apps x supported profiles x text scales

27 cells = 9 supported profiles (kobo-profile SUPPORTED_PROFILES) x 3 text scales. Reproduce: `scripts/quality/census-matrix.sh`. Portrait cells; landscape is app-owned via SetOrientation. 758x1024 is not a supported profile.

| cell | pass/total | failed apps |
|---|---|---|
| clara-bw-391-default | 45/45 | - |
| clara-bw-391-large | 44/45 | gallery |
| clara-bw-391-extra-large | 43/45 | fieldbook, gallery |
| clara-bw-395-default | 45/45 | - |
| clara-bw-395-large | 44/45 | gallery |
| clara-bw-395-extra-large | 43/45 | fieldbook, gallery |
| clara-hd-376-default | 45/45 | - |
| clara-hd-376-large | 44/45 | gallery |
| clara-hd-376-extra-large | 43/45 | fieldbook, gallery |
| clara-colour-393-default | 45/45 | - |
| clara-colour-393-large | 44/45 | gallery |
| clara-colour-393-extra-large | 43/45 | fieldbook, gallery |
| elipsa-2e-389-default | 44/45 | gallery |
| elipsa-2e-389-large | 44/45 | gallery |
| elipsa-2e-389-extra-large | 44/45 | gallery |
| libra-2-388-default | 44/45 | gallery |
| libra-2-388-large | 44/45 | gallery |
| libra-2-388-extra-large | 45/45 | - |
| libra-colour-390-default | 44/45 | gallery |
| libra-colour-390-large | 44/45 | gallery |
| libra-colour-390-extra-large | 45/45 | - |
| libra-colour-390-4.46.23836-default | 44/45 | gallery |
| libra-colour-390-4.46.23836-large | 44/45 | gallery |
| libra-colour-390-4.46.23836-extra-large | 45/45 | - |
| libra-h2o-384-default | 44/45 | gallery |
| libra-h2o-384-large | 44/45 | gallery |
| libra-h2o-384-extra-large | 45/45 | - |

## Failure index by app

- fieldbook: clara-bw-391-extra-large, clara-bw-395-extra-large, clara-hd-376-extra-large, clara-colour-393-extra-large
- gallery: clara-bw-391-large, clara-bw-391-extra-large, clara-bw-395-large, clara-bw-395-extra-large, clara-hd-376-large, clara-hd-376-extra-large, clara-colour-393-large, clara-colour-393-extra-large, elipsa-2e-389-default, elipsa-2e-389-large, elipsa-2e-389-extra-large, libra-2-388-default, libra-2-388-large, libra-colour-390-default, libra-colour-390-large, libra-colour-390-4.46.23836-default, libra-colour-390-4.46.23836-large, libra-h2o-384-default, libra-h2o-384-large

## Provenance

Cells ran 2026-09-18/19 at HEADs 27201967..6e4ba702; those commits touch only
`scripts/quality/census-matrix.sh`, so every cell measured identical app,
library and simulator code (verified: `git diff` over crates/, apps/,
examples/, tools/ is empty across the two cell SHAs). Per-cell SHAs are in
`census.json`.
