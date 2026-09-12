# Miniflux account policy

The RSS Reader can now install a `miniflux` token through the SDK's server-bound
account entry. The private record contains the token and its HTTPS server in one
atomic write. This is platform support; the app's new account-entry flow is still
being implemented. The token is resolved only inside the runtime.

Requests must match the saved origin, port and base path, the verified
`rss-miniflux` app identity, the account name and the `X-Auth-Token` header.
The policy permits only these operations beneath the configured base path:

| Method | Route | Limits |
| --- | --- | --- |
| GET | `/v1/me`, `/v1/version`, `/v1/feeds`, `/v1/categories` | No body, content type or query |
| GET | `/v1/entries` | Required limit 1–100; optional bounded offset, status, starred, order and direction; no repeated/unknown parameters |
| GET | `/v1/entries/{id}` | One positive exact ID; no query |
| GET | `/v1/entries/{id}/fetch-content?update_content=false` | Fetch content without replacing the server copy |
| PUT | `/v1/entries` | 1–100 distinct positive IDs and an explicit status, starred value, or both |
| POST | `/v1/feeds` | HTTPS feed URL and optional positive category ID; no other fields |

JSON bodies are limited to 8 KiB. Duplicate keys, unknown fields, invalid types,
IDs outside the JSON parser's exact integer range, URL fragments and escaped API
routes are refused. Feed creation cannot attach extra usernames, passwords or
proxy options. Toggle-bookmark, account administration and other mutation routes
are not authorized. The old POST-to-entries grant was removed because Miniflux
requires PUT. Legacy unbound unread fetching remains compatible, but new writes
require a server-bound account.

The runtime checks the full saved-account policy before resolving a token, even
if a host callback would otherwise allow a broader request. An invalid saved
record cannot fall back to a legacy secret. Panels and calibre-web remain limited
to their existing read-only Basic-account rules.

The request contracts come from the [official Miniflux API reference](https://miniflux.app/docs/api.html).
Explicit starred values require Miniflux 2.3.2 or later. The app must read back
and reconcile uncertain changes rather than retrying a toggle or trusting that an
older server understood an optional field. Feed creation likewise requires
reconciliation after a lost reply to avoid duplicate subscriptions.

## Validation · 9 September 2026

140 policy tests pass. New cases cover valid routes, changed server/port/base path,
wrong app/name/header, duplicate and unknown fields, query bounds, full-content
read semantics, feed payloads and unbound writes. A task-runner test installs a
private bound record and verifies the exact method, body and resolved token at
the transport boundary, then refuses another server and an unreviewed field even
with an intentionally permissive callback.

Strict all-target policy Clippy and Rust 1.85.1 ARMv7 musl compilation of the native
runtime with `device-write` pass. No new external library was added; policy uses
the workspace's existing JSON crate. These are platform tests, not an actual
Miniflux server or finished app journey. Durable articles, queued-change recovery,
account setup UI, full reader integration and updated app screenshots remain in
progress. No Miniflux checklist item is marked complete by this change.
