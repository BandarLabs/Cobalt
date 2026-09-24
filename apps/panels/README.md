# Panels

Read CBZ comics from your computer or your [Komga](https://komga.org/)
library.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/library.png" alt="The shelf, with covers and saved pages"><br>The shelf, with covers and saved pages</td>
<td width="50%" valign="top"><img width="300" src="screenshots/reader.png" alt="Reading a page"><br>Reading a page</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/import-guide.png" alt="The USB import guide"><br>The USB import guide</td>
<td width="50%" valign="top"><img width="300" src="screenshots/download-recovery.png" alt="A finished download, ready to add offline"><br>A finished download, ready to add offline</td>
</tr>
</table>

## Features

- Fit, zoom, pan, thumbnails, page jump, rotation, two-page spreads and
  right-to-left reading.
- A shelf of up to 64 comics with covers and your saved page, such as
  **Saved page 2 of 4**.
- Browse and search a Komga library, and download comics up to 32 MiB.
- Interrupted downloads can be continued or removed from **Download**.
- **Try a sample comic** opens *A small garden*, a four-page comic made for
  Cobalt.

## Adding a comic over USB

1. Choose **Add comic** for the guide.
2. Connect the reader by USB and copy your comic to
   `.adds/cobalt/data/panels/volume.cbz`. The folder is hidden, and you may
   need to create `panels`.
3. Eject the drive, unplug the cable and reopen Panels.
4. Choose **Add comic**, check the title and size, then **Add to library**.

Panels keeps its own verified copy, so you can copy the next comic to
`volume.cbz` without losing the last one. **Available on this reader** appears
once the copy is saved.

## Connecting Komga

1. Choose **Browse Komga** and enter your server's HTTPS address. Paths such
   as `https://books.example/komga` work.
2. Open **Account details** and enter your username and password.

Panels checks the address before saving it. Your account is stored by the
runtime for that server only. Changing servers means entering your details
again.

## Limits

- CBZ only. CBR and PDF comics are not supported, so convert CBR files to CBZ
  first.
- Continuing a download re-checks the part already saved, which uses some
  extra data.

## Permissions

- `network`: browses and downloads from your Komga server.

## Development

```sh
cargo test -p kobo-panels
python3 scripts/check-apps-sim.py panels
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`. `scripts/quality/check-comics-sim.py` checks importing, reading and save recovery. `scripts/quality/make-panels-sample.py` rebuilds the sample comic.

## Credits

Panels is unofficial and not affiliated with or endorsed by the Komga project.
CBZ files are read with the `zip` crate (MIT). The sample comic's art and text
are original.
