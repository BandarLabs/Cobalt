# Birds

Inspired by the awesome [fugleramme](https://github.com/arnegiacomo/fugleramme) project by Arne Giacomo Munthe-Kaas. Its artwork-first, quiet e-ink design is the reason this companion exists.

Birds is an offline Kobo viewer for a microphone and BirdNET-Go running on a Mac or Linux computer. The Kobo has no microphone and never runs the model. The collage fills the reading surface; its bird names are part of Fugleramme's rendered plate. On a colour Kobo, Birds keeps and paints the source RGB through Cobalt's colour-picture path; greyscale models decode only luminance.

```sh
kobo setup --enable-ssh
kobo birds listen --source http://garden-computer.local:8080 --device 192.168.1.42
kobo birds status
kobo birds stop
```

<img width="300" src="screenshots/birds.png" alt="A labelled collage of public-domain bird plates filling the Birds app on a Kobo">
<img width="300" src="screenshots/birds-colour.png" alt="The same bird collage rendered in RGB for a Kobo Clara Colour">

`--source` is Fugleramme's local web endpoint. Fugleramme polls BirdNET-Go, renders the collage, and exposes `/state` plus `/collage.png`; the companion pushes a new snapshot only when that state token changes. Transfer uses Cobalt's established owner-attended SSH route. Each publication writes the collage to a content-addressed image file first and commits `current.json` last as the pointer, so the app sees either the old complete snapshot or the new one; an interrupted publish never overwrites the image the old snapshot still names, and orphaned images are pruned by the next successful publish.

If the host, microphone or network disappears, the last complete page remains. A snapshot older than one day is marked stale. Refresh reopens local shelf files; it does not turn on Wi-Fi. Fugleramme does not expose recent detections as machine-readable JSON, so the automatic bridge leaves that optional list empty; a manually prepared `kobo birds push SNAPSHOT.json IMAGE.png` snapshot may include it.

Native Windows is out of scope. Cobalt's host CLI currently relies on Unix process and filesystem behavior.

## Licenses

No Fugleramme source, artwork, fonts, BirdNET-Go binary, or BirdNET model is bundled in this app. The companion is an API client and transfer tool written for Cobalt.

- Fugleramme code is MIT. If downstream work copies its code, retain the notice in `licenses/FUGLERAMME-MIT.txt`.
- Fugleramme classic artwork is CC BY-SA 4.0 and is not bundled. Users who add it to a distribution must carry its per-image manifest and attribution.
- The checked-in fixture collage and screenshots are a composite of public-domain 19th-century ornithological plates from Wikimedia Commons; `THIRD-PARTY.md` lists every source plate, and `scripts/fixtures/birds/build-collage.py` rebuilds the fixture.
- BirdNET-Go and its BirdNET model are separately distributed under CC BY-NC-SA 4.0 for non-commercial use. They are not part of the Cobalt package.

See `THIRD-PARTY.md` and `licenses/`.

## Validation

The real model-to-screen acceptance chain is recorded in [`docs/quality/birds-e2e.md`](../../docs/quality/birds-e2e.md). Default and extra-large text-scale screenshots are checked in under `screenshots/`. Physical-Kobo acceptance remains open.
