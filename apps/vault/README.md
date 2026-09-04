# Vault

A read-only, offline reader for Obsidian vaults. It keeps browsing, note text,
tags, backlinks, and a case-insensitive search on the reader. It is unofficial
and not affiliated with Dynalist Inc.; the UI is named simply **Vault**.

```sh
kobo vault init --device 192.168.1.42
kobo vault push ~/Notes --device 192.168.1.42
```

For the simulator, push the same folder into the host store the app reads:

```sh
kobo vault init --sim
kobo vault push ~/Notes --sim
```

`kobo sync setup DIR --folder vault` copies files onto the reader under
`sync/vault`. Vault does not read that folder. It loads one packed index
(`vault-index-v1`) from the app store, so the companion that writes that key
is `kobo vault push`.

<img width="300" src="screenshots/home.png" alt="Vault on a Clara BW explaining how to push a vault">

## Dependencies

The app quarantines `pulldown-cmark` (MIT) in `src/md.rs` and sends its HTML
through Cobalt's `kobo-html` renderer. Wiki links (`[[Note]]`) resolve to
notes in the packed index. PDF, audio, video, canvas, and image attachments
are not rendered; they are skipped when packing. The packed index must fit
in the 256 KB app-store value limit.
