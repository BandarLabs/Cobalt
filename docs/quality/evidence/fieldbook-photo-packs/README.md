# Fieldbook photo packs

`kobo fieldbook photos` enriches an existing version 1 Fieldbook shelf with real
320 px JPEGs from Avicommons. It writes a version 2 directory containing
`packs.v1`, `attribution.json`, and content-addressed files under `photos/`.
The reader therefore needs no network connection in the field.

The importer accepts only CC0, CC BY, and CC BY-SA records. It refuses NC, ND,
and unknown licenses. Every photo reference must have a complete matching
attribution record with creator, license, source links, byte count, and SHA-256.
`inspect` and `push` fail if a reference, attribution, digest, size, or JPEG is
missing or inconsistent.

Sources:

- Avicommons integration and URL contract: https://github.com/rawcomposition/avicommons
- Pinned 2025 catalog: https://avicommons.org/2025.json
- Avicommons project site: https://avicommons.org/

## Live proof

The live run used the existing 12-species GBIF Central Park shelf. Avicommons
supplied six eligible photos totaling 82,609 bytes. The complete offline pack
was 89,498 bytes. License mix: one CC BY-SA 3.0, one CC BY 2.0, three CC0 3.0,
and one CC0 2.0. Six species had no eligible record and stayed usable without a
photo. The first fetch took 1.6 seconds; the same build from the local cache
took 0.1 seconds. An exact eight-file simulator copy matched the generated pack.
Removing one attribution record made `inspect` reject the pack.
