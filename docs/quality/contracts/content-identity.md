# Contract: content identity, import and export

One identity per content item across the platform; adoption is staged and
atomic; export is complete and deterministic.

## Identity and provenance

- Content identity is a hash of the original bytes plus a normalized
  metadata key. Identical bytes arriving from two connectors resolve to
  one identity with multiple provenance records.
- Same filename with different bytes is a conflict, never a silent
  duplicate.
- Import validates MIME, size and path before adoption; oversized,
  unsupported, malformed and path-traversal inputs are rejected at the
  boundary.

## Adoption

- Import is staging plus atomic commit: failure at any phase removes
  temporary files and leaves the library untouched.
- App-contributed content appears in shared views only after successful
  adoption.
- Removing an adapter never deletes adopted content without explicit
  confirmation.
- Existing app-owned content stays readable during migration to the shared
  foundation; the migration adapter is part of the contract, not an
  afterthought.

## Export bundle

- Original file + normalized metadata + reading state + annotations +
  provenance, with checksums per part.
- Conflicting destination filenames receive deterministic numbered
  suffixes.
- Export retries never delete or mutate the reader's original.
- Restore shows a preview (compatible / skipped / conflict) before
  applying anything.
- Named secret values are never exported - only references and the setup
  steps required to re-establish them.

## Tests

- Identical bytes from two sources; same name/different bytes;
  malformed/oversized/traversal rejection; interruption at every phase;
  migration readability; export checksum round-trip; deterministic
  conflict naming.
