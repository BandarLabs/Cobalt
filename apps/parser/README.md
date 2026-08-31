# Parser

A small offline interactive-fiction MVP for the Kobo panel.

![Original story after taking the lamp and entering the garden](screenshots/parser-game.png)

The library and transcript conventions are ready for a standards-compliant Z-machine core, but this
first playable build ships an original two-room story rather than a Z-machine interpreter. Tap the
command rows or use the platform keyboard; every accepted turn changes the transcript and score.

## License and format notes

The eventual Z-machine implementation will follow [Z-Machine Standards 1.1](https://inform-fiction.org/zmachine/standards/z1point1/index.html).
Commercial Infocom stories are not bundled; readers must sideload copies they are entitled to use.
No third-party story is distributed in this MVP.

## Capabilities

`network` is reserved for the specified TLS sideload endpoint; gameplay remains offline. The MVP has
not yet implemented transfer, Quetzal saves, v3/v5/v8 execution, or conformance-suite support.
