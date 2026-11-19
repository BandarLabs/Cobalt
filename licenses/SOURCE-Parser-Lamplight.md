# Lamplight test fixture provenance

`apps/parser/fixtures/lamplight.z3` is a small original Z-machine story
written for Cobalt as Parser's test fixture, replacing a third-party
story so the fixture's provenance is entirely in our own tree.

- Story source: `apps/parser/fixtures/lamplight.inf`, written for this
  project and held in this repository.
- Library: PunyInform 6_8 by Fredrik Ramsberg and Johan Berntsson, MIT,
  https://github.com/johanberntsson/PunyInform, licence copied to
  `licenses/LICENSE-PunyInform.txt`.
- Compiler: Inform 6.44, https://github.com/DavidKinder/Inform6.
  Build: `inform6 -v3 +include_path=<punyinform>/lib lamplight.inf`
- Facts: Z-machine version 3, release 1, serial 260919, checksum
  0x276d, 28,160 bytes, SHA-256
  155badb9c3ce9e6772a7451203da9e34df947d4d518e0484771c553d0fe439e9.
- Compiled 2026-09-19. The file is a test input only; it is not bundled
  into the application or distributed to readers.
