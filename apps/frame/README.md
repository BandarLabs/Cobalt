# Frame

Turn a Kobo into a low-power black-and-white photo frame.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/frame.png" alt="A photo filling the screen"><br>A photo filling the screen</td>
<td width="50%" valign="top"><img width="300" src="screenshots/companion-preview.png" alt="Comparing crop and pad on the computer"><br>Comparing crop and pad on the computer</td>
</tr>
</table>

## Features

- Two modes:
  - **Frame mode** keeps the reader awake and changes photo every 5, 15 or 60
    minutes.
  - **Slow slideshow** wakes the reader every 1, 6 or 24 hours to change the
    photo, and lets it sleep in between.
- Shuffle or date order. Settings and the current position are remembered.
- Tap either side to move between photos. Tap the centre for the photo's file,
  album, date and transfer state.
- Every photo is checked against the digest recorded when it was sent.
  Changed or damaged photos are skipped and listed on the home screen.
- Each photo change uses a full-quality screen refresh.

Frame does not replace the Kobo's own sleep screen.

## Setup

Frame receives photos over the SSH connection set up by
`kobo setup --enable-ssh`. Prepare and send photos from your computer:

```sh
kobo frame init --device 192.168.1.42
kobo frame push ~/Pictures/family --device 192.168.1.42
kobo frame push portrait.jpg --fit pad --device 192.168.1.42
kobo frame ls --device 192.168.1.42
kobo frame rm photo-0123456789abcdef --device 192.168.1.42
```

- JPEG, PNG, GIF and WebP are accepted, and camera orientation is applied.
- Photos are cropped to fill the screen by default. `--fit pad` keeps the
  whole photo with white borders.
- `push` adds to the album. `--album NAME` names the album; otherwise the
  folder name is used. Photos already on the reader are not sent again.
- `--delete` replaces the album and removes photos not in the new set.
- Every command accepts `--sim` to try it on the simulator.

### Preview and plan

```sh
kobo frame preview ~/Pictures/family --out ./frame-preview
kobo frame plan ~/Pictures/family --device 192.168.1.42 --album "Summer holiday"
```

`preview` writes an `index.html` comparing crop and pad for each photo, for
the Clara BW or another reader with `--profile`. `plan` lists what a push
would add, keep and remove without changing anything. Run it with `--delete`
before a replacement.

### Undo

```sh
kobo frame restore --device 192.168.1.42
```

Before any change to a non-empty album, Frame saves a copy of it. `restore`
brings back the album as it was before the last push or removal. It keeps one
step of history, not an archive, and needs up to 300 MB of free space.
Keep your originals on your computer.

A push reports `Frame transfer verified` only after reading the album back
from the reader and checking every photo.

## Limits

- Up to 500 photos and 150 MB of prepared images.
- Source files up to 32 MB and 50 million pixels.
- HEIC and HEIF are not supported. Convert them to JPEG first.
- Photos are shown in greyscale.

## Permissions

- `keep-awake`: keeps the screen on in Frame mode.
- `scheduled-wake`: wakes the reader to change photos in Slow slideshow.

## Development

```sh
cargo test -p kobo-frame
python3 scripts/check-apps-sim.py frame
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`.

## Credits

Images are prepared and decoded with the `image` crate (MIT or Apache-2.0).
The sample photograph is [Blue Marble](https://svs.gsfc.nasa.gov/30613) by the
NASA Johnson Space Center Earth Science and Remote Sensing Unit.
