# Fieldbook companion acceptance - real GBIF pack and eBird CSV

The actual `kobo fieldbook` CLI validated and atomically pushed a pack derived
from public GBIF occurrence results for a 5 km search around Central Park. The
running Fieldbook app read all 12 species, searched the pack, logged a Veery
sighting in a named outing, and wrote the checklist. The CLI then received the
15-row file without changing the app's original.

GBIF API query:
https://api.gbif.org/v1/occurrence/search?decimalLatitude=40.78&decimalLongitude=-73.97&distance=5&classKey=212&limit=100

- Raw response SHA-256: `b8d63d08438e0471c0a5e644f15af864d0a53975e3dda8f359e9fe5622e29d28`
- Derived `packs.json` SHA-256: `a7cf2e0dc5e4ad72e606569804573cd43b24c91b038688288938353a5d95e58e`
- Pack result: one pack, 12 species, zero failures.
- Export result: columns A/B hold species/common name from row 15 and outing
  checklists begin in column C, matching eBird Checklist Format.

The five 1072x1448 frames were pixel-inspected. They show the CLI-delivered
pack count, real pack title, a Magnolia Warbler search result, the logged
Veery, and the export page. The CSV is included beside them. This validates
the simulator and local host receiver, not a physical reader or SSH transfer.
