# Readeck

Read and search saved articles from your HTTPS Readeck instance. Mark articles
read, archive, favorite/unfavorite, or confirm deletion. Changes appear only
after the server acknowledges them; writes are not automatically retried.

## Publication dependency

This draft requires the Readeck credential policy in
[PR #270](https://github.com/BandarLabs/Cobalt/pull/270).
The manifest minimum `999.0.0` is a development guard, not a real release.
Before merging/publishing, replace it with the first Cobalt release containing
that policy, regenerate the app pages, and complete the contributor dry-run.
Protocol compatibility alone does not grant permission to use Readeck tokens.

## Setup from a computer

Create a Readeck API token with bookmark read/write access. Close Cobalt normally
and wait for teardown. Keep the Kobo awake on Wi-Fi, with owner SSH configured.
Using the CLI from this change, run:

```sh
kobo secret set readeck --app readeck --server https://readeck.example \
  --from /path/to/private-token --device <reader-address>
```

The file must contain only the token and be private (`chmod 600`). Alternatively,
on macOS copy the token and run:

```sh
pbpaste | kobo secret set readeck --app readeck --server https://readeck.example \
  --stdin --device <reader-address>
```

Do not put tokens in command-line arguments or shell history. Clipboard history
or sync tools may retain them. The command neither prints the token nor checks
it against Readeck. It atomically installs a private server-bound account, not
app settings. If interrupted, an occupied `.server-account-setup.lock` needs
inspection before retrying; do not blindly delete it.

Open Readeck on the Kobo, enter the **same HTTPS server address**, and choose
**Use saved token**. Subpaths are supported, for example
`https://reader.example/readeck`. A different server cannot use that token.

## Setup on the Kobo

Enter the HTTPS address, tap **Next**, and enter the token in the masked account
form. The runtime stores the account; the app saves only the server address.
The inbox opens after both save replies.

Use **More → Account** to replace the account or select a computer-saved token.
Replacing credentials does not revoke the old token: revoke unused tokens in
Readeck settings. Missing or invalid tokens require setup again.

## Reading and storage

- Inbox: latest 50 unread/reading article or photo bookmarks.
- Search: includes read and archived articles, paged in batches of 50.
- Reader: article actions, text size, and frontlight controls.
- Reading positions: saved per server and case-sensitive article ID.
- Images: ordinary same-origin reader fetches, without bearer tokens. Images
  requiring authentication will not load.
- No offline article library, OAuth, or synced annotations in this version.

Credentials live in `secrets/apps/readeck/servers/readeck`. App state contains
only the server address, reading size, and article positions. The standard
updater preserves secrets, state, and installed Store apps. Official
installation requires the signed Store package and compatible platform release.

## Checks

```sh
cargo test -p kobo-readeck -p kobo-policy
cargo test -p kobo-cli server_secret -- --test-threads=1
node tools/app-contribute.mjs --manifest apps/readeck/cobalt-app.json --print-plan
```

Tests cover account setup, failed saves, reading positions, search, mutations,
images, and Clara/Elipsa layouts. They are not fresh hardware acceptance.
Before publication, test the final signed package on a Kobo and verify that an
update preserves the account and reading position.
