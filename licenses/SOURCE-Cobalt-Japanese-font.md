# Corresponding source for the Cobalt Japanese font subset

`crates/kobo-flashcards-format/fonts/CobaltJapanese-Regular.otf` is a bounded
subset derived from `NotoSansCJKjp-Regular.otf` in the Noto CJK repository:

- Repository: <https://github.com/notofonts/noto-cjk>
- Revision: `165c01b46ea533872e002e0785ff17e44f6d97d8`
- Source path: `Sans/OTF/Japanese/NotoSansCJKjp-Regular.otf`
- Source SHA-256:
  `68a3fc98800b2a27b371f2fb79991daf3633bd89309d4ffaa6946fd587f375b5`
- Derived font SHA-256:
  `150c82a7b6a4e39645099b3d27c96a00a148a1f57faf523027559910059c2dc0`

The subset contains Latin-1, the JIS X 0208 repertoire, and the standardized
JIS X 0213 plane-one additions in rows 1–15 and 90–94. It is intentionally
smaller than Cobalt's existing 2 MiB local font-transfer bound. Text requiring
another glyph is rejected by the host converter instead of being rendered as
an empty box.

Rebuild it with Python and `fonttools==4.25.0`:

```sh
git clone https://github.com/notofonts/noto-cjk.git
git -C noto-cjk checkout 165c01b46ea533872e002e0785ff17e44f6d97d8
python -m pip install fonttools==4.25.0
python scripts/build-flashcards-japanese-font.py \
  noto-cjk/Sans/OTF/Japanese/NotoSansCJKjp-Regular.otf \
  crates/kobo-flashcards-format/fonts/CobaltJapanese-Regular.otf
```

The modified font uses the family name **Cobalt Japanese**. Its licence is
shipped in `licenses/LICENSE-Cobalt-Japanese-font.txt`.
