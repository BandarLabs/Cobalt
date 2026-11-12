# kobo-frame-host

Host-only image preparation and manifest logic for `kobo frame`. It accepts
JPEG, PNG, GIF, and WebP; applies EXIF orientation; fits each photo to the
reader's panel (1072×1448 by default) by crop or pad; and maintains the
deterministic, durable Frame manifest. A greyscale reader gets a grey PNG,
reduced on the computer; a colour reader keeps the colour, as a JPEG.

HEIC/HEIF is intentionally not supported. Adding libheif bindings would add
LGPL considerations, so v1 asks the owner to convert to JPEG or PNG instead.

| Library | License | Where |
| --- | --- | --- |
| `image` | MIT OR Apache-2.0 | Decoding, orientation, resizing, and PNG and JPEG encoding |
| `blake3` | CC0-1.0 OR Apache-2.0 | Stable content identities in the manifest |
