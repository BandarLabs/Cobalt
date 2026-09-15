# Contract: app quality manifest

Every installable app declares a manifest the Store, runtime and reviewer
can all check. The manifest is the per-app contract referenced by release
gates; it lives in the app's `kobo.toml` and is rendered into signed
catalog rows.

## Required fields

- **Primary user and job** - who it serves and the one job it does.
- **Offline promise** - exactly what works with no network, stated as
  user-visible behavior, not a flag.
- **User-created data** - what the user can make, where it lives, and the
  export format for each kind (EPUB/PDF/CBZ originals, Markdown, JSON,
  CSV, ICS, OPML or service-native identifiers).
- **Data retention on remove** - per data kind: retained, exported-then-
  deleted, or deleted; the uninstall flow shows this before acting.
- **Required capabilities** - app fails without them; each with a
  purpose sentence.
- **Optional capabilities** - app degrades gracefully; each with the
  feature it gates.
- **Supported profiles** - by profile ID from the
  [simulator matrix](simulator-matrix.md), with per-pose notes.
- **Maintainer and support link** - a named owner and an issue
  destination.
- **Explicit non-goals** - what the app deliberately does not do.

## Rules

- Capabilities are described as purposes in user-facing language, not only
  technical names.
- The Store shows permission additions during update; unchanged
  permissions are not re-prompted.
- No app enters Beta because its directory exists; it enters when its
  manifest is complete and its release gate passes
  (see [channels](channels.md)).

## Tests

- Manifest schema parse/validate in CI; catalog rows are generated from
  manifests, never hand-edited.
- Store UI journeys render purposes, retention policy and update diffs
  from the manifest.
