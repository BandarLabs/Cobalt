# Zotero Reader

Zotero Reader is a read-only Cobalt app for browsing personal Zotero
collections and reading papers on a Kobo. Google Scholar can remain an input
through the Zotero Connector; the app never accesses or scrapes Scholar.

The public Store build reads metadata, abstracts, stored-PDF attachment
records, and Zotero-indexed text directly from Zotero Web API v3. It makes no
Zotero write requests. It also supports multiple collections, local search,
offline text, reading position, and annotations.

![Zotero Reader displaying a paper on a Kobo](screenshots/reading.png)

## Set up direct Zotero access

1. Open [Zotero API key settings](https://www.zotero.org/settings/keys).
2. Create a dedicated key with read-only access to your personal library. Do
   not grant write access.
3. Note the numeric user ID displayed on the same page.
4. Install the key under the exact secret name `zotero`:

   ```sh
   kobo secret set zotero --device <address>
   ```

5. Launch Zotero Reader, enter the numeric user ID, and select a collection.
   Use **Collections** in the feed header whenever you want to switch.

The runtime—not the app—stores and attaches the key. Its platform policy binds
the `zotero-reader` app identity and `zotero` secret to reviewed read-only
routes on `https://api.zotero.org`. The app never receives the secret value.

Direct mode does not need another server. Zotero's indexed full-text endpoint
returns plain text, so this mode cannot preserve the original PDF layout,
tables, figures, formulas, or OCR structure.

## Structured PDF conversion service

The self-hosted conversion service is crucial when you want the full academic
reading experience: structured headings, tables, figures, formulas, and OCR
from PDFs stored in Zotero. It downloads only stored Zotero attachments,
converts them with Docling, and returns bounded reader-safe HTML and figures.

The current service is single-user and self-hosted; it is not a shared cloud
backend bundled with the Store app. Start with these public resources:

- [Project and service source](https://github.com/andreclerigo/cobalt-zotero-reader)
- [Reader setup guide](https://andreclerigo.github.io/cobalt-zotero-reader/setup.html)
- [Self-hosting guide](https://andreclerigo.github.io/cobalt-zotero-reader/self-hosting.html)
- [Bridge API and trust boundaries](https://andreclerigo.github.io/cobalt-zotero-reader/api.html)

The bridge deployment requires a dedicated read-only Zotero key, an
allowlisted collection, a randomly generated bearer token, HTTPS, persistent
derived-content storage, and the supplied Docker Compose stack. Keep every
credential outside Git.

Cobalt deliberately refuses to send stored credentials to arbitrary hosts.
The default signed Store package therefore uses direct mode. Enabling a
self-hosted bridge currently requires the custom Cobalt integration described
by the project documentation: compile the exact bare HTTPS origin through
`ZOTERO_READER_BRIDGE_ORIGIN` and install that service's bearer token under the
exact name `zotero-bridge`. The origin must be `https://host` with no path,
user information, or alternate port.

## Permissions and privacy

Zotero Reader requests only:

- `network` for Zotero and an explicitly authorized conversion service;
- `frontlight-control` for the reading view.

The direct `zotero` key is sent only to approved Zotero read routes. A bridge
uses a separate `zotero-bridge` token; the bridge keeps its own Zotero key, so
the device's direct Zotero key is never sent to that service. Credentialed
redirects are refused.

The committed fixtures are synthetic and contain no account data, credentials,
or private papers.

## Development

```sh
cargo test -p kobo-zotero-reader
cargo run -p kobo-cli -- run --sim --app zotero-reader
```
