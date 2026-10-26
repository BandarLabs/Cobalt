# Parser companion real-story proof

The host CLI inspected and pushed the compiled Zork I story from
[`historicalsource/zork1`](https://github.com/historicalsource/zork1) at commit
`97b7b3d68c075dd9af7da499c3e9690ada3471fd`. That repository licenses the work
under MIT. `source.json` records the exact artifact hash and parsed header.

The running Parser app used the same `kobo-zstory` inspector, listed the CLI
shelf file, opened it, and ran the built-in LOOK action. `library.png`,
`opened.png`, and `look.png` are ideal simulator captures from that journey.
The first attempt exposed fixed measured-layout and latest-page regressions in
the app; these retained captures are from PR-A `c3a2dc20`, after both fixes.

Pixel inspection found no clipping or overlap. The library identifies the
85 KB story. The opened and post-LOOK screens show actual Zork location text,
reachable page turns, keyboard, suggestions, and save controls. Because the
current location description is unchanged by LOOK, the proof asserts the
visible `pile of leaves` sentence before and after the action instead of
claiming a content change the game does not make.

This is simulator proof. It does not validate physical-reader rendering,
touch, storage, or transfer.
