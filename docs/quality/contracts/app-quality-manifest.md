# Contract: app quality manifest

Every installable app declares a manifest the Store, runtime and reviewer
can all check. The manifest is the per-app contract referenced by release
gates; it lives in the app's existing `cobalt-app.json`, is validated by
`tools/app-registry.mjs`, and is rendered into signed catalog rows by
`tools/collect-app-registry.mjs`. No parallel manifest file is introduced.

## Required fields

- **Primary user and job** (`user`, `job`) - who it serves and the one
  job it does.
- **Offline promise** (`offline`) - exactly what works with no network,
  stated as user-visible behavior, not a flag.
- **User-created data** (`data`) - one entry per kind: `kind`,
  `location`, and the `export` format (EPUB/PDF/CBZ originals, Markdown,
  JSON, CSV, ICS, OPML or service-native identifiers). A kind that is
  not exported says so and why. An explicit empty array declares the
  app creates no user data.
- **Data retention on remove** (`data[].on_remove`) - per data kind:
  `retained`, `exported-then-deleted`, or `deleted`; the uninstall flow
  shows this before acting.
- **Required capabilities** (`capabilities_required`) - app fails
  without them; each entry names the capability and a `purpose`
  sentence. An explicit empty array declares the app needs none.
- **Optional capabilities** (`capabilities_optional`) - app degrades
  gracefully; each entry names the capability and the feature it
  `gates`. An explicit empty array declares there are none.
- **Supported profiles** (`profiles`) - by profile ID from the
  [simulator matrix](simulator-matrix.md), with per-pose notes.
- **Maintainer and support link** (`maintainer`, `support`) - a named
  owner and an issue destination.
- **Explicit non-goals** (`non_goals`) - what the app deliberately does
  not do.

## Rules

- Completeness is all or nothing: declaring any quality field declares
  all of them, and a half-finished manifest fails validation with the
  missing fields named. Manifests without the fields still validate, so
  the rollout stages app by app.
- A capability named in `capabilities_required` or
  `capabilities_optional` must also appear in the manifest's existing
  `capabilities` list, so the user-facing story cannot drift from what
  the runtime enforces.
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
