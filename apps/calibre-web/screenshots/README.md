# calibre-web screenshots

`catalog.png`, `private-catalog.png`, `reading.png` and `repair.png` are SDK
simulator captures on Clara BW, taken through the app's own routes with
public-domain books. `libraries.png` is the empty library list a new reader
opens on. `setup-retry.png` is an older capture of a refused setup save and
predates the current controls.

`scripts/quality/check-calibre-sim.py` plays the longer HTTPS OPDS/EPUB journey
against a local test server. Its fixture verifies authenticated requests
without recording account headers.
