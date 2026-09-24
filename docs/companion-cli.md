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

## Companion commands by app

Apps that need files, accounts or a paired computer have a `kobo` command.
Run `kobo COMMAND --help` for the full usage. Most take `--device IP` for a
reader or `--sim` for the simulator.

| App | Command | What it does |
| --- | --- | --- |
| [Birds](../apps/birds/README.md) | `kobo birds listen`, `status`, `stop`, `push` | Sends Fugleramme collages to the reader |
| [Deck](../apps/deck/README.md) | `kobo deck init`, `set`, `ls`, `show`, `push` | Builds and sends the pad layout |
| [Feeds](../examples/rss/README.md) | `kobo feeds check`, `push` | Checks and sends an OPML subscription list |
| [Fieldbook](../apps/fieldbook/README.md) | `kobo fieldbook inspect`, `photos`, `push`, `ls`, `export` | Prepares field packs and fetches checklists |
| [Flashcards](../apps/flashcards/README.md) | `kobo flashcards import`, `verify`, `preview`, `stage`, `export-review-log` | Converts Anki packages and copies them over USB |
| [Frame](../apps/frame/README.md) | `kobo frame preview`, `init`, `plan`, `push`, `restore`, `ls`, `rm` | Prepares and sends photo albums |
| [Music Stand](../apps/musicstand/README.md) | `kobo musicstand init`, `plan`, `push`, `ls`, `rm` | Sends scores from PDF or images |
| [Needles](../apps/needles/README.md) | `kobo needles setup`, `preview`, `prepare`, `push` | Converts and sends knitting patterns |
| [Nonograms](../apps/nonograms/README.md) | `kobo nonograms preview`, `push` | Turns photos into puzzles |
| [Panels](../apps/panels/README.md) | `kobo panels inspect`, `preview`, `push` | Checks and sends CBZ comics |
| [Paperterm](../apps/paperterm/README.md) | `kobo stream init`, `demo`, `pairing`, `terminal`, `monitor` | Shares a terminal session with the reader |
| [Parser](../apps/parser/README.md) | `kobo parser inspect`, `push` | Checks and sends story files |
| [Post](../apps/post/README.md) | `kobo post login` | Connects Post to a Hermes gateway |
| [Read Later](../apps/readlater/README.md) | `kobo readlater login` | Signs in to Wallabag |
| [Sidekick](../examples/sidekick/README.md) | `kobo-sidekickd init`, `setup`, `run`, `sample` | Runs the daemon that forwards agent prompts |
| [Sync](../apps/syncthing/README.md) | `kobo sync setup`, `run`, `plan`, `status`, `publish`, `pause`, `resume`, `stop` | Runs a private Syncthing peer |
| [Vault](../apps/vault/README.md) | `kobo vault init`, `plan`, `push`, `ingest`, `preview`, `ls`, `rm` | Sends notes from a folder |

Commands every app can use:

- `kobo secret set NAME --from FILE --device IP` installs an API key or
  token. The runtime attaches it to the app's requests, and the app never sees
  it.
- `kobo trust set NAME --from ROOT.pem --device IP` installs a private
  certificate authority. For Sidekick and Paperterm, `--from` can be left out
  because their `init` commands save the certificate where `trust set` looks.
- `kobo export --app APP --device IP --out FOLDER` receives a copy an app has
  prepared with its export or **Save a copy** button.
- `kobo apps setup APP` shows an app's setup steps.

## Sync folders

`kobo sync setup LOCAL_DIR --folder vault|frame|books|out --device IP` pairs one
computer folder with the reader's fixed Sync set through a dedicated, private
Syncthing peer (its own home, its own identity, API on loopback only).
`kobo sync plan` shows the mapping, its fixed direction and what the reader
imports from it; `kobo sync run` starts the peer, `status` reports folder
state with last change and errors, `pause` and `resume` suspend transfers
with the reader, and `stop` shuts the peer down. `kobo sync publish --folder
vault` packs raw Markdown notes into the shelf package the reader imports
after each window; the owner's original files are never modified. Set
KOBO_SYNC_HOME to an absolute path to keep a separate configuration.

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

After pairing Sidekick, try `kobo-sidekickd sample` with the normal daemon
stopped. It sends a built-in connection-check question to the reader. Choose
**Received** to confirm delivery to the computer; press Ctrl-C to stop.
No agent setup or command execution is involved.

## Deck layouts

A Deck layout push preserves existing pairing on the reader and simulator.
On a fresh simulator, `kobo deck push --sim` opens a static preview. Pads show
that they cannot run commands; choose **Pair** to connect to the computer.
A device push updates the cached grid and never substitutes preview pairing
for a real computer connection. See the [Deck guide](../apps/deck/README.md)
for preview screenshots.

For each Deck pad, `kobo deck set ... --confirm` enables confirmation and
`--no-confirm` disables it. Editing an existing pad without either flag keeps
its current setting. A new pad defaults to no confirmation. Supplying both
flags is an error and leaves configuration unchanged.
