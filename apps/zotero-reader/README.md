# Stacks

Browse your Zotero collections and read papers on your Kobo.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/reading.png" alt="Reading a paper with its Zotero details"><br>Reading a paper with its Zotero details</td>
</tr>
</table>

## Features

- Browse several collections. **Collections** in the header switches between
  them.
- Read each paper's details, abstract and Zotero's indexed full text.
- Search locally, keep papers available offline, and keep your reading
  position and annotations.
- Read-only. Stacks never changes your Zotero library.

## Setup

1. Open [Zotero's API key settings](https://www.zotero.org/settings/keys).
2. Create a key with read-only access to your personal library, and note your
   numeric user ID on the same page.
3. Install the key on the reader:

   ```sh
   kobo secret set zotero --device <address>
   ```

4. Open Stacks, enter your user ID and choose a collection.

The runtime keeps the key and sends it only to Zotero's read-only API routes.
The app never sees it.

## Limits

Zotero's indexed full text is plain text, so the original PDF layout, tables,
figures and formulas are lost.

For structured papers with headings, tables, figures and formulas, a
self-hosted conversion service is available at
[andreclerigo/cobalt-zotero-reader](https://github.com/andreclerigo/cobalt-zotero-reader).
It needs a custom build of Cobalt, because the Store build only sends
credentials to Zotero. See its
[setup guide](https://andreclerigo.github.io/cobalt-zotero-reader/setup.html)
and [self-hosting guide](https://andreclerigo.github.io/cobalt-zotero-reader/self-hosting.html).

## Permissions

- `network`: reads your Zotero library.
- `frontlight-control`: adjusts the front light while reading.

## Development

```sh
cargo test -p kobo-zotero-reader
cargo run -p kobo-cli -- run --sim --app zotero-reader
```

The test fixtures are synthetic and contain no account data.

## Credits

Stacks is unofficial and not affiliated with or endorsed by Zotero or the
Corporation for Digital Scholarship, which owns the Zotero trademark.
