# kobo-frame-host

Host-only image preparation and manifest logic for `kobo frame`. It accepts
JPEG, PNG, GIF, and WebP; applies EXIF orientation; produces bounded 1072×1448
greyscale PNGs with crop or pad fitting; and maintains the deterministic,
durable Frame manifest.

HEIC/HEIF is intentionally not supported. Adding libheif bindings would add
LGPL considerations, so v1 asks the owner to convert to JPEG or PNG instead.

| Library | License | Where |
| --- | --- | --- |
| `image` | MIT OR Apache-2.0 | Decoding, orientation, resizing, and PNG encoding |
| `blake3` | CC0-1.0 OR Apache-2.0 | Stable content identities in the manifest |
