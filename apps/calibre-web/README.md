# calibre-web library

Browse your library's OPDS catalog, download books and keep reading offline.
The app follows the server's sections, authors, shelves and book links. Back
returns to the previous catalog page and keeps its place.

## Connect a library

Choose **Add library** and enter its HTTPS OPDS address. A bare server address
opens `/opds`; an explicit path is used as entered, including reverse-proxy paths.
Choose **Sign in** for a private library or **Use public library** otherwise.

Sign in asks for your username and password separately. The runtime stores the
account with the selected HTTPS server; the app never reads it back. Account use
is limited to GET requests on that server. Changing servers requires signing in
again. Existing installations that used `kobo secret set calibre` should sign in
on the reader to enable authenticated subpages and book downloads: the legacy
unbound credential remains limited to the catalog root.

Use a trusted HTTPS certificate. For a private certificate authority, install
its trust root with `kobo trust set calibre --device <address>`. For `calibre serve`,
use Basic authentication; Digest and the calibre-web Kobo-sync endpoint are not
supported.

Library setup is saved after a successful catalog check. If that write fails,
**Retry saving setup** preserves the checked address. Unreadable settings remain
untouched and offer **Retry loading settings**. Downloaded books remain available.

## Read offline

Open a book's details and choose **Download**. EPUB and plain-text books use the
shared BookView reader, with page navigation, text size, bookmarks and retained
reading position. **Downloaded books** opens without fetching the server.

Files are limited to 16 MB and the local library to 64 books. Downloads can be
cancelled and restarted. The library adds a file only after its shelf write is
acknowledged; the full SHA-256 digest verifies local content before opening.
Identical content reuses the local file. A damaged file offers **Download again**,
keeping its existing library entry until the replacement is saved.

Library and reading-position writes are serialized and acknowledged. Failed
writes offer **Retry saving**; corrupt or future library records are preserved.

![Catalog sections](screenshots/catalog.png)
![Offline reading](screenshots/reading.png)
![Recovering a damaged download](screenshots/repair.png)

## Validation

The original OPDS/EPUB fixtures are in `fixtures/`. Build `kobo-cli`, then run:

```sh
python3 scripts/quality/check-calibre-sim.py --output /tmp/calibre-check
```

The route uses an isolated, locally trusted HTTPS server and private simulator
storage. It checks navigation, download, forced restart, offline reading, failed
saves, repair, on-screen setup and authenticated requests. It does not contact
an owner's server. Hardware acceptance is scheduled separately.
