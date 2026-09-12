# Use Cobalt from your computer

Run `kobo` in a terminal to choose an owner task by number:

1. Set up a reader over USB. Connect the reader and choose **Connect** on its
   screen. Setup identifies the device and keeps its existing installation
   review and approval step.
2. Preview photos for Frame. Enter a photo or folder path, then a new output
   folder. Open the generated `index.html` to compare crop and pad. Previewing
   does not transfer photos.
3. Check a feed subscription file. Enter an OPML export path to see its feed
   count and subscription addresses. Checking does not contact those sites or
   transfer anything.
4. Check a Paperterm connection. This starts the built-in typing check using
   your saved pairing identity. For first-time configuration, run `kobo stream`
   to see the pairing instructions.
5. Open the developer and release command reference.
6. Read an app setup guide. Choose an app by number to see its setup steps,
   links and the reader capabilities it requests.

Choose **0** or press Enter at a blank prompt to cancel. Paths can contain
spaces; enter the path itself without shell quotes. The menu passes it
literally to the same command used by scripts.

If input or output is redirected, bare `kobo` prints compact help and exits
without prompting. Explicit commands are unchanged. `kobo --help` opens the
full command reference; `kobo frame --help`, `kobo flashcards --help` and
`kobo feeds --help` show companion-specific commands.

Named-reader selection, a desktop companion window and resumable operation
receipts remain under development.

## Feed subscription files

`kobo feeds check FILE` and `kobo feeds push FILE --sim` accept OPML files up
to 256 KB. The CLI reads at most the limit plus one byte before refusing an
oversized export; export a smaller selection if needed. Validation happens
before the destination is changed.

Simulator staging writes and syncs a temporary file before publishing it. If
the temporary filename is already occupied, the operation fails and preserves
both that file and the existing subscription list. Device transfers already
publish through a temporary file. Neither operation claims the subscriptions
or article content are available offline: open Feeds and import the staged
list, then download the articles you want to keep.

The [Feeds validation transcript](quality/evidence/feeds-companion/transcript.json)
shows an original sample import and failure recovery using an isolated
simulator shelf. No feed URLs are fetched during this check.

## App setup guides

Run `kobo apps` to list the bundled apps, or narrow the list with
`kobo apps search chess`. Open a guide with `kobo apps setup lichess`.
The same guides are available through option **6** in the numbered menu.

Each guide uses the app's published manifest for its setup instructions,
account links and commands. Requested capabilities are explained in plain
language, including Wi-Fi, frontlight control and keeping the reader awake.
Apps without extra setup steps show the Store installation instruction.

These guides work offline and describe the catalog bundled with your CLI.
They do not inspect your reader, verify an account, run the displayed commands
or mark setup steps complete. Replace placeholders such as `<address>` before
running a command yourself.

![Lichess setup guide, rendered from the verified CLI output](quality/evidence/app-guides/lichess-guide.svg)

## Credentials and certificates

Use `kobo secret --help` for credential commands and `kobo trust --help` for
private-server certificate commands. Help exits successfully without looking
for local credentials or connecting to a reader. Missing required arguments
still return an error.

Choose exactly one destination: `--device ADDRESS` for a reader over the
network, or `--volume PATH` for its mounted USB volume. Repeated or mixed
destinations are refused. Use `--from PATH` once with `set`; it is not accepted
with `list` or `remove`. Credential values belong in a private file, not in the
command line. For the app's required token scope and provider link, open its
setup guide, for example `kobo apps setup lichess`.

Credential replacement first writes a temporary file beside the existing
credential, then publishes it. Invalid input and failed writes preserve the
previous value. If another attempt already owns the staging file, the command
refuses to overwrite it. Local input is limited to 4 KB and must be a regular
text file. New staging files request private permissions; permission support
depends on the USB volume's filesystem. This does not certify power-loss
recovery on a physical reader.

## Sidekick integrations

Run `kobo-sidekickd setup` in a terminal to see supported integrations and
choose one by number. Enter or **0** cancels. Redirected input displays status
and explicit commands without configuring anything. Use
`kobo-sidekickd setup AGENT --dry-run` to preview a selected integration, or
`--print` to obtain configuration for manual setup. Read the
[Sidekick companion guide](../crates/kobo-sidekickd/README.md) for pairing and
listener setup.

![Sidekick integration chooser, rendered from terminal output](quality/evidence/sidekick-setup/selection.svg)

After pairing Sidekick, try `kobo-sidekickd sample` with the normal daemon
stopped. It sends a built-in connection-check question to the reader. Choose
**Received** to confirm delivery to the computer; press Ctrl-C to stop.
No agent setup or command execution is involved.

![Sidekick sample on the reader simulator](quality/evidence/sidekick-sample/default/sample-question.png)
