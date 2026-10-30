# Credential routes declared by the app

A proposal. Nothing in it is enforced yet; it describes how the credential
policy would move from the platform's code into each app's signed manifest
without weakening what the policy guarantees.

## What the policy guarantees today

An application never holds a token. It names a stored secret in a request,
and the runtime decides whether that secret may be attached to that exact
request before it resolves the value. The decision is default-deny and binds
the runtime-verified app id to the credential's name, its header convention,
the request kind, the HTTPS origin, the path, the query, and for a write the
shape of the body. `kobo-policy/src/credentials.rs` is where that decision
lives.

## Why it has to move

The decision is written per app and per service, in the platform:

- `credential_servers::may_set` lists which app may store which server-bound
  secret: Panels and Komga, Calibre-Web and Calibre, Digest and Miniflux,
  Read Later and Wallabag, Readeck, Post and its gateway.
- `allowed_request` names Lichess, Zotero, the Audiobook studio's Exa key and
  the Chat app's three providers inline.
- Miniflux, Readeck and Post each have a module of reviewed routes beside it.

So a new app that talks to a service needs a platform release, the policy
crate grows a module per service, and an app's manifest, the one document an
owner and a reviewer can read, says nothing about where its token may go.
The roadmap's "owner-approved service connections" is this problem.

## The proposal

`cobalt-app.json` gains a `credentials` list. It travels in the signed
package manifest, so it is fixed at install, re-verified at every launch, and
a change to it is a release input like a capability.

```json
"credentials": [
  {
    "name": "miniflux",
    "header": { "named": "X-Auth-Token" },
    "server": "owner",
    "routes": [
      { "methods": ["GET"], "path": "/v1/feeds" },
      { "methods": ["GET"], "path": "/v1/entries",
        "query": ["status", "limit", "after_entry_id", "order", "direction"] },
      { "methods": ["PUT"], "path": "/v1/entries", "body": "json",
        "max_body": 8192,
        "fields": { "entry_ids": "ids", "status": ["read", "unread"] } }
    ]
  }
]
```

- `name` and `header` must match the credential the app presents, exactly as
  the modules check them now. `header` is `bearer`, `basic` or
  `{ "named": "..." }`.
- `server` is `owner`, meaning the HTTPS origin the owner entered when the
  secret was stored and nothing else, or a fixed origin such as
  `https://api.exa.ai` for a public provider.
- A route names its methods, an exact path or one with integer segments
  (`/v1/entries/{id}`), an allowlist of query parameters, and for a write the
  body kind, a byte ceiling and a small field grammar: a list of ids, an
  enumeration, or a bounded string. Anything the grammar cannot say is
  refused, and nothing outside the listed routes is reachable.

The runtime keeps one evaluator for this shape and uses it for every app. The
simulator runs the same evaluator, so a route that is refused on the reader is
refused in `kobo dev`.

## What stays in the platform

- URL hygiene: HTTPS only, no fragments, no control characters, no
  downgrade on redirect. These are properties of the transport, not of an app.
- The server-binding store and the rule that an owner-entered server is read
  back with the secret as one record.
- The body grammar's interpreter and its ceilings. The grammar grows only by a
  platform change with a test, never by an app asking.
- Caps: routes per credential, path and query lengths, body sizes. Unknown
  keys fail closed, as the manifest does everywhere else.
- The built-in apps' fixed-provider entries, which `builtin_declared` already
  owns.

## Migration

1. Add the field to the manifest schema in `tools/app-registry.mjs` and to
   the signed manifest in `kobo-app-store`. A package that carries it derives
   a `minimum_cobalt_version` that includes the evaluator.
2. Write each existing per-service policy as a manifest entry, and prove
   parity: the existing tests in `credentials.rs` and the three service
   modules become a table run against both the old function and the new
   evaluator. The old code is deleted only when every row agrees.
3. Keep the hard-coded table for an installed app whose manifest predates the
   field, until that app's next release carries one. The runtime already
   distinguishes the two by the manifest it verified at install.
4. Show the routes on the app's page beside its capabilities, so an owner can
   read where a token may go before entering one.

## Risks

- Grammar creep. A service that needs a shape the grammar lacks is a platform
  change with a review, not a new escape hatch; the list of shapes should stay
  short enough to read in one sitting.
- Over-broad routes approved in review. A `credentials` diff is a release
  input with its own line in the contributor check's output, and a reviewer
  reads it the way they read a capability.
