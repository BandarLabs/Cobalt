# Readeck

Read and search saved articles from your HTTPS Readeck instance. Mark articles
read, archive, favorite/unfavorite, or confirm deletion. Changes appear only
after the server acknowledges them; writes are not automatically retried.

## Requirements

Requires Cobalt **0.3.25 or newer**, the first release containing the Readeck
credential policy from [PR #270](https://github.com/BandarLabs/Cobalt/pull/270).
Protocol compatibility alone does not grant permission to use Readeck tokens.
Laptop setup additionally requires a CLI with the `secret set --app --server`
options introduced alongside this app; older CLIs do not support them.
On-device token entry does not require the new CLI.

## Setup

1. Set up the reader once over USB, if you have not already: `kobo setup`.
2. In Readeck, create an API token that can read and update bookmarks.
3. Connect from your computer:

   ```sh
   kobo connect readeck
   ```

   It asks for the HTTPS server (subpaths such as
   `https://reader.example/readeck` work) and the token, typed without being
   shown. It checks the token against Readeck, installs it bound to that one
   server, and saves the server in the app. Cobalt can stay open. Open
   Readeck, or tap **Check again** if it is already open.

With the token in a private file (`chmod 600`), the same thing is one
command, which also checks the token and saves the server:

```sh
kobo secret set readeck --app readeck --server https://readeck.example --from TOKEN_FILE
```

## Setup on the Kobo

Enter the HTTPS address, tap **Next**, and enter the token in the account
form. The form counts the characters typed and has **Show** to read the token
back once. The runtime stores the account; the app saves only the server.

Use **More, Account** to replace the account. Replacing credentials does not
revoke the old token: revoke unused tokens in Readeck settings.

## Offline and failures

Articles you open are kept on the reader (the most recent ones, up to 240 KiB
each), and the last inbox is kept too, so both open without Wi-Fi. A failed
refresh keeps the inbox on screen with a banner saying why: a refused token
offers **Sign in again**, a server at home that does not answer says the
reader must be on the home Wi-Fi, and no Wi-Fi offers **Join Wi-Fi**.

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
After merge, Beta publishing creates the official signed Store package.
Before Stable promotion, install that package on a Kobo and verify that an
update preserves the account and reading position. Local contributor previews
use a public test key and must not be trusted as official Store releases.

The [listing image](screenshots/inbox.png) uses synthetic articles rendered by
Cobalt at Clara BW dimensions. It is not device-test evidence. Its reproducible
capture is in `tools/review-captures/apps/readeck/`.
