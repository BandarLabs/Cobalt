# Panels

Panels reads local CBZ comics and catalogs from your own Komga library. Comic
archive inspection, page decoding and reading controls are shared Cobalt
components. ZIP decoding uses the pinned MIT-licensed `zip` crate with default
features disabled. There is no RAR decoder or CBR dependency. CBR and PDF comics
are outside the current supported formats; obtain a CBZ copy of a CBR comic.

## Add and read a local comic

Send a CBZ file as `volume.cbz` in Panels' shelf directory, then choose **Add
comic**. Check its title, format and size, then choose **Add to library**. Panels
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

## Current server integration

Komga requests use the runtime's HTTP Basic credential named `komga`:

```sh
kobo secret set komga --device <address>
```

The current catalog endpoint remains `https://komga.local/opds/v1.2/catalog`.
An editable server setup, complete interrupted-download validation and progress
summaries on the shelf remain open quality tasks. Existing server browsing
supports nested catalogs, filtering the current page, cover art and bounded
CBZ downloads. Recovery data is retained until a completed comic's library save
succeeds. Do not treat this as a guarantee of every interrupted-download case.

```sh
cargo test -p kobo-panels
kobo dev
```

Run `kobo dev` from this app directory. The repository's
`scripts/quality/check-comics-sim.py` uses original geometric fixtures in private
simulator storage and checks imports, reading, save failure, retry and restart.
Physical Clara BW validation follows the combined three-PR acceptance run.
