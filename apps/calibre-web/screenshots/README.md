# calibre-web screenshots

`catalog.png`, `reading.png`, `repair.png`, `setup-retry.png` and
`private-catalog.png` are actual SDK simulator captures of calibre-web 0.1.3 on
Clara BW at Extra-large. They come from the isolated HTTPS OPDS/EPUB journey in
`scripts/quality/check-calibre-sim.py`. Matching layouts, source/binary/font
provenance and the 16-check result are in `docs/quality/evidence/calibre-web`.

The fixture verifies authenticated requests without recording account headers.
Earlier `libraries.png` and `address.png` retain the empty-setup route captures.
Physical hardware acceptance is scheduled after the three quality PRs.
