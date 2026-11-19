# Birds end-to-end evidence

Validated on Linux on 2026-09-16. No fake detection was used in this chain.

1. BirdNET-Go 20260823 loaded the embedded BirdNET v2.4 FP32 model and its range filter. Its benchmark completed at 44 ms average per inference with XNNPACK and 65 ms on the standard CPU path in this two-core sandbox.
2. MediaMTX accepted BirdNET-Go's repository-owned `tawnyowl.wav` sample as a looping RTSP audio source. BirdNET-Go classified it as **Tawny Owl / Strix aluco** at confidence 1.0 and saved the detection through its own database/API. The first accepted API row was detection id 1 at `2026-09-16T13:28:40+05:30`.
3. Fugleramme 0.21.3 polled that real BirdNET-Go API, selected `strix-aluco-2.webp`, and rendered a 1440x1080 RGB collage. Its state token was `9d8792db6114bc40`; the PNG SHA-256 was `052c016abe9ef615ea8dace7bb98589bdfa38963a879f219ac46ea377c682081`.
4. `kobo birds listen --source http://127.0.0.1:8096 --sim --interval 2` started a real background poller, and `kobo birds status` reported it running. The exact Fugleramme PNG landed in the simulator shelf with the same SHA-256.
5. The audio publisher was then changed to BirdNET-Go's repository-owned `soundscape.wav`. BirdNET-Go produced a new detection (including Common Scoter), Fugleramme changed its state token to `7e90787381f09093` and rendered a new PNG (`acef04f4b62550ed7741f8b71ad798bd255bd0441f6024e5a457f89bf2c4f5e1`). Without restarting the companion, its next poll atomically replaced the simulator snapshot and image with that exact new hash. The companion log contained two successful publications.
6. The Birds app opened the real listener shelf. Both default and extra-large text-scale drive routes passed and produced the checked-in screenshots.

7. After the full-screen rework, the same real stack was restarted. BirdNET-Go accepted fresh Tawny Owl detections from `tawnyowl.wav` (latest id 29), then the RTSP publisher switched to `soundscape.wav`; BirdNET-Go accepted new Red Crossbill, Hawfinch, Canada Goose, and Merlin detections (ids 31-35). Fugleramme's portrait token changed from `5b6f399bac16cc23` to `4c22d588be070e07`; its second collage hash was `c1ce46ef6b6dfa9448d6f864238bf5754fe8df119b7987237782ea13dce632ce`, exactly matching the listener's simulator-shelf image. App captures before and after differ in 652,059 pixels over `(90, 188)-(982, 1323)`, with SHA-256 changing from `0fabc449...` to `4147de35...`. This directly proves the new full-screen app updates after live polling, rather than only proving transfer.

This proves model execution, real audio classification, BirdNET-Go persistence/API, Fugleramme polling and artwork rendering, companion interval polling, atomic handoff, and app rendering. It is a host/simulator acceptance run, not physical-Kobo proof. The sandbox has no microphone device, so a repository-owned WAV was published as a real-time RTSP stream instead of pretending to test live capture.

### Full-screen and colour verification

The full-screen view has no app top bar, timestamp row, or action bar: Fugleramme's labelled plate is the screen. A centre tap opens the small status/refresh/exit overlay; side taps refresh the local shelf. The app covers the source to the panel and asks the SDK for the reader identity before selecting its picture path. Greyscale readers decode and upload the luminance plane. Colour models decode RGB and use `put_colour_picture`; the same upload still carries a derived luminance plane for monochrome rendering.

Fresh default (100%) and extra-large (140%) Clara BW captures each completed three paints from the real listener shelf. Across the full 1072 x 1448 panel they contain 1,223,268 and 1,223,739 pixels below grey 245, respectively, with a 0-255 range. Typography scale changes only the hidden overlay, so the art remains the dominant full-panel sheet.

A Clara Colour simulation also completed three paints via the colour path. The RGB capture is 1072 x 1448, its red-green and green-blue difference bounds both span `(60, 116)-(1011, 1401)`, and its channel means are 214.99 / 209.23 / 197.41. This proves actual chroma reached the Cobalt surface, not merely a colour-capable profile drawing the grey upload. Physical panel appearance remains uncalibrated simulator evidence.

A broad `cargo test --workspace` attempt compiled through the wider workspace but the host linker was killed by signal 9 while linking a large test binary. The focused Birds app/CLI tests, strict Clippy, catalog suite, companion integration, simulator routes, and offline reopen are the passing acceptance evidence; the broad workspace run is not claimed as passing.
