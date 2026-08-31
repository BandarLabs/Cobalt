# Vault

A read-only, offline reader for Obsidian vaults. It keeps browsing, note text,
tags, backlinks, and a case-insensitive search on the reader. It is unofficial
and not affiliated with Dynalist Inc.; the UI is named simply **Vault**.

```sh
kobo vault init
kobo vault push ~/Notes
```

<img width="300" src="screenshots/home.png" alt="Vault on a Clara BW explaining how to push a vault">

## Dependencies

The app quarantines `pulldown-cmark` (MIT) in `src/md.rs` and sends its HTML
through Cobalt's `kobo-html` renderer. The companion host transfer planned by
the specification uses `obsidian-export` (BSD-2-Clause), but it cannot be
included in this app-only commit. PDF, audio, video, and canvas files are not
rendered by this reader.
