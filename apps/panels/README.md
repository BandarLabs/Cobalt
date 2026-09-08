# Panels

Panels reads local CBZ comics and catalogs from your own Komga library. Comic
archive inspection, page decoding and reading controls are shared Cobalt
components. ZIP decoding uses the pinned MIT-licensed `zip` crate with default
features disabled. There is no RAR decoder or CBR dependency. CBR and PDF comics
are outside the current supported formats; obtain a CBZ copy of a CBR comic.

## Add and read a local comic

Choose **Try a sample comic** to read the bundled four-page *A small garden*
offline. Its artwork and text are original to Cobalt, and its import follows the
same confirmation and receipt flow as any other CBZ.

To add your own file, choose **Add comic** for the USB guide. Connect the reader
by USB, open its drive on your computer, and copy your CBZ as `volume.cbz` under
`.adds/cobalt/data/panels` (show hidden folders and create `panels` if needed).
Eject the drive safely, unplug the cable, reopen Panels and choose **Add comic**. Check its title, format and size, then choose **Add to library**. Panels
keeps a content-addressed copy and verifies its bytes before saving a receipt
and the comic list. **Available on this reader** appears only after both saves
succeed. Choose **Open** to read, or return to the shelf to read later.

A repeated import reuses an intact copy. Replacing the incoming `volume.cbz`
does not replace previous content-addressed imports. The shelf supports up to
64 comics, with measured pages and stable selection at each interface size.
Unreadable, corrupt and newer-version comic lists remain distinct from an
empty shelf. Failed saves offer a retry and retain the pending data.

Reading controls include fit, zoom, pan, thumbnails, page jump, rotation,
spreads and right-to-left reading. Position changes wait for a storage
acknowledgement; a failed save remains visible until a retry succeeds. New
position records use bounded content-derived keys, with legacy positions read
when available. The library accepts the previous tab-separated format through
an acknowledged migration.

## Connect your Komga library

Choose **Browse Komga**, add your HTTPS server address, then open **Account
details** and enter your username and password. Cobalt stores the account in
Panels' private runtime namespace, bound to that server and its base path.
A reverse-proxy path is supported, for example `https://books.example/komga`.
The connection check requests `/opds/v1.2/catalog` beneath that address and opens
the library only after a valid OPDS response. Login pages and generic server
errors are refused. An address save failure offers a retry; reopening restores
the last acknowledged address. Changing servers requires entering account
details for the new server.

Browsing paginates each server response at the active interface size, including
its navigation and next/previous links. Search filters the current response while
preserving each original title action. Back restores the previous local page and
search; cancelled or late requests cannot replace that restored view. Catalog
history is bounded to 16 entries. Cover art and bounded CBZ downloads remain
available. Interrupted-download validation and shelf progress summaries remain
open quality tasks. Recovery data
is retained until a completed comic's library save succeeds; this does not yet
cover every interrupted-download case.

```sh
cargo test -p kobo-panels
kobo dev
```

The sample can be rebuilt with `python3 scripts/quality/make-panels-sample.py`.
It uses original drawing geometry and the repository's existing licensed font.
The source, PNG pages and CBZ use the repository license; no external artwork
or comic text is embedded.

Run `kobo dev` from this app directory. The repository's
`scripts/quality/check-comics-sim.py` uses original geometric fixtures in private
simulator storage and checks imports, reading, save failure, retry and restart.
Physical Clara BW validation follows the combined three-PR acceptance run.
