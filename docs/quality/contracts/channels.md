# Contract: release channels

## Semantics

- **Stable**: installable by default. Only packages present in the signed
  Stable catalog are shown. Promoted from the exact immutable Beta-tested
  artifacts, never rebuilt.
- **Beta**: opt-in per channel setting. Platform channel and app catalog
  channel are separate choices; enabling one never enables the other.
  Returning from Beta to Stable preserves compatible apps, state and
  secrets references.
- **Developer/Local**: sideloaded or workspace-built packages. Always
  labelled; never mixed into signed-catalog availability claims.

## Availability truth

Store and website availability is derived only from signed release
metadata. Branch presence, directory presence in `apps/`, and committed
screenshots are not availability. At base `4bb0d0e` the signed catalog
(`apps/catalog.json`) is empty while 33+ app directories exist: nothing is
installable until a signed catalog row exists.

## Presentation

- Channel badges (Stable/Beta/Developer) are distinguishable in monochrome
  without relying on colour.
- Changing channel asks for explicit confirmation and explains rollback.
- An invalid or expired catalog signature keeps the last verified catalog
  and raises a durable error; it never falls back to an empty store or to
  unsigned data.
- An update row shows old version, new version, permission additions,
  compatibility result and estimated bytes.

## Tests

- Signed/invalid/expired catalog fixtures under `scripts/fixtures/catalog/`.
- Store simulator journeys: default Stable-only view, Beta opt-in, invalid
  signature durability, rollback copy.
- Website/catalog generation consumes only the signed release candidate.
