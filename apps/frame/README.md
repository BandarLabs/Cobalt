# Frame

Frame presents photographs pushed from a computer as a low-power e-ink photo
frame. This Store MVP uses the platform picture cache and demonstrates both
`keep-awake` frame mode and `scheduled-wake` slow-slideshow mode. Every image
change is intended to receive a full panel refresh; idle photographs consume
no panel power.

<img width="300" src="screenshots/frame.png" alt="A monochrome sample photograph shown in the Clara BW simulator">

The image shown is an included sample. `kobo frame init` and `kobo frame push`
require the separate host transfer service and are not included in this
app-only delivery, so no upload is falsely claimed. The home screen makes that
limit explicit.

## Dependencies

| Library | License | Where |
| --- | --- | --- |
| `image` | MIT OR Apache-2.0 | Planned host/device pipeline via `kobo-image` |

Frame is monochrome by design. The current runtime picture budget cannot hold
one 1072×1448 greyscale image, so this MVP deliberately uses a 536×724
portrait instead of falsely presenting a blank “full-bleed” frame. A production
transfer service needs tiled panel pictures before it can fulfil that part of
the specification. It will decode, EXIF-orient, fit, and halftone source
images before storing them.
