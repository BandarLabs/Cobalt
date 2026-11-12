<p align="center">
  <img src="docs/logo.svg" width="220" alt="Cobalt">
</p>

<p align="center"><strong>Apps and an SDK for Kobo e-readers.</strong></p>

<p align="center">
  <a href="https://github.com/BandarLabs/Cobalt/actions/workflows/ci.yml"><img src="https://github.com/BandarLabs/Cobalt/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/BandarLabs/Cobalt/actions/workflows/apps.yml"><img src="https://github.com/BandarLabs/Cobalt/actions/workflows/apps.yml/badge.svg?branch=main" alt="Publish apps"></a>
  <a href="https://github.com/BandarLabs/Cobalt/releases/latest"><img src="https://img.shields.io/github/v/release/BandarLabs/Cobalt?color=brightgreen" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/BandarLabs/Cobalt?color=brightgreen" alt="License"></a>
</p>

Cobalt adds apps to Kobo e-readers. It includes a launcher, an app store, a
Rust SDK, a runtime that runs each app in its own restricted process, and a
simulator for building apps without a device.

Install Cobalt once over USB. After that, apps install, update and uninstall
over Wi-Fi, and your Kobo's own reader stays as it was.

<p align="center">
  <a href="https://bandarlabs.github.io/Cobalt/">Website</a> ·
  <a href="https://bandarlabs.github.io/Cobalt/install/">Install</a> ·
  <a href="https://bandarlabs.github.io/Cobalt/#apps">Apps</a> ·
  <a href="https://bandarlabs.github.io/Cobalt/sdk.html">SDK</a> ·
  <a href="https://www.reddit.com/r/CobaltForKobo/">Community</a>
</p>

<p align="center">
  <a href="docs/cobalt-tour.mp4">
    <img src="docs/tour.gif" height="600" alt="A Kobo Clara BW paging through the Cobalt launcher, playing the opening of a chess game against the computer in Lichess, then opening a crossword">
  </a><br>
  <sub>Recorded on a Kobo Clara BW running Cobalt 0.3.35, with only the waits shortened.</sub>
</p>

> [!IMPORTANT]
> Tested on the Kobo Clara BW (N365 and the 2025 P365), Clara Colour, Clara HD,
> Elipsa 2E, Libra 2, Libra Colour and Libra H2O, at the firmware listed in the
> [device support matrix](docs/DEVICES.md#device-support-matrix). Other Kobos
> can run it too: Cobalt lists what has not been tested and asks once before
> it starts. Kobo firmware 5.x is not supported. Cobalt is not affiliated with
> Rakuten Kobo.

## Install

The easiest way is the [browser installer](https://bandarlabs.github.io/Cobalt/install/),
in Chrome, Edge or Opera. Plug in your Kobo and follow the steps.

Or, on macOS or Linux:

```sh
curl -fsSL https://bandarlabs.github.io/Cobalt/install.sh | sh
```

Then restart the reader, wait a minute, and open **Cobalt** from the Kobo menu.
If you already use NickelMenu, Cobalt is added to it and your other entries
are kept.

The script also turns on the reader's SSH server, with a key only this
computer holds as the way in, and saves the reader when it comes back on
Wi-Fi. Apps that take files or accounts from your computer, such as Frame,
Music Stand and Vault, then work with commands like
`kobo frame push ~/Pictures/family` and nothing else to set up. If you used
the browser installer, run the script above once with the reader plugged in
when an app first needs your computer; it keeps everything already on the
reader. `kobo setup --no-ssh` leaves SSH off.

Everything the script downloads is checked against the signed release
manifest. To verify `install.sh` itself first, follow the
[signed-bootstrap procedure](docs/INSTALL.md#signed-bootstrap).
[docs/INSTALL.md](docs/INSTALL.md) covers updates, recovery, uninstalling and
building from source.

### Updates

- **Cobalt** updates from **Settings** on the reader. Settings also switches
  between the Stable and Beta channels, keeping your apps and data.
- **Apps** update from **App Store**.
- **The `kobo` command** on your computer updates with `kobo update`, or
  `kobo update --channel beta`. This never changes the reader.

## Apps

Install and remove apps from **App Store** on the reader, or from an app's
page on the website: tap **Link a browser** in App Store to link a phone or
computer, then use **Install** on any
[app page](https://bandarlabs.github.io/Cobalt/#apps). The launcher, App
Store, Settings and Terminal come with Cobalt and cannot be removed.

Screenshots are from a Kobo Clara BW or its simulator.

<!-- store-apps:start -->
<table>
<tr>
<td width="33%" valign="top"><a href="examples/audiobook/README.md"><img width="230" src="docs/media/site/apps/audiobook.png" alt="An audiobook player with cover art and playback controls on a Kobo"></a><br><b><a href="examples/audiobook/README.md">Audiobooks</a></b><br>Play the audiobooks on your Kobo, with chapters, speed, a sleep timer and your place kept. Or make one from a topic.</td>
<td width="33%" valign="top"><a href="apps/backgammon/README.md"><img width="230" src="docs/media/site/apps/backgammon.png" alt="Backgammon on a Kobo in the middle of a game against the computer, with its last move marked on the board and described below it."></a><br><b><a href="apps/backgammon/README.md">Backgammon</a></b><br>Play complete solo or pass-and-play backgammon on one Kobo.</td>
<td width="33%" valign="top"><a href="apps/birds/README.md"><img width="230" src="docs/media/site/apps/birds.png" alt="A labelled collage of public-domain bird plates filling a Kobo screen."></a><br><b><a href="apps/birds/README.md">Birds</a></b><br>Show the birds heard by BirdNET-Go on your Mac or Linux computer. Needs your computer.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="examples/chat/README.md"><img width="230" src="docs/media/site/apps/chat.png" alt="An answer displayed for touch-friendly reading on a Kobo"></a><br><b><a href="examples/chat/README.md">Chat</a></b><br>Ask a question, then read and navigate the answer with touch controls. Needs an account.</td>
<td width="33%" valign="top"><a href="examples/gallery/README.md"><img width="230" src="docs/media/site/apps/components.png" alt="Cobalt typography and interface components on a Kobo"></a><br><b><a href="examples/gallery/README.md">Components</a></b><br>See every Cobalt UI component on the device in one reference app.</td>
<td width="33%" valign="top"><a href="apps/crossword/README.md"><img width="230" src="docs/media/site/apps/crossword.png" alt="Crossword grid on a Kobo with an answer filled in, the current clue above it and every clue below it."></a><br><b><a href="apps/crossword/README.md">Crossword</a></b><br>Sixty-one offline crosswords with clue navigation and saved progress.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="examples/brief/README.md"><img width="230" src="docs/media/site/apps/brief.png" alt="A numbered daily news brief on a Kobo"></a><br><b><a href="examples/brief/README.md">Daily Brief</a></b><br>A daily news brief that fetches a fresh one when you open it and keeps every story for offline reading.</td>
<td width="33%" valign="top"><a href="apps/deck/README.md"><img width="230" src="docs/media/site/apps/deck.png" alt="Deck paired with a computer, showing Test, Format, Lint, Docs and Deploy command pads."></a><br><b><a href="apps/deck/README.md">Deck</a></b><br>Turn your Kobo into a remote control for your computer. Needs your computer.</td>
<td width="33%" valign="top"><a href="apps/rss-miniflux/README.md"><img width="230" src="docs/media/site/apps/rss-miniflux.png" alt="An essay from Walden open in Digest on a Kobo, under an engraving of Thoreau's house."></a><br><b><a href="apps/rss-miniflux/README.md">Digest</a></b><br>Read your Miniflux feeds anywhere. Needs an account.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/fanshelf/README.md"><img width="230" src="docs/media/site/apps/fanshelf.png" alt="A followed work in Fanshelf naming its author, fandom, rating and chapter count, with a Download EPUB button."></a><br><b><a href="apps/fanshelf/README.md">Fanshelf</a></b><br>Save public AO3 works to a shelf made for offline reading.</td>
<td width="33%" valign="top"><a href="examples/rss/README.md"><img width="230" src="docs/media/site/apps/feeds.png" alt="Unread articles from one subscribed feed in the Feeds app on a Kobo"></a><br><b><a href="examples/rss/README.md">Feeds</a></b><br>Follow feeds, search saved articles and read offline with images.</td>
<td width="33%" valign="top"><a href="apps/fieldbook/README.md"><img width="230" src="docs/media/site/apps/fieldbook.png" alt="Fieldbook during an outing at Lake Merced, listing the birds logged so far with a Finish outing button."></a><br><b><a href="apps/fieldbook/README.md">Fieldbook</a></b><br>Log bird sightings anywhere and build your life list.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/flashcards/README.md"><img width="230" src="docs/media/site/apps/flashcards.png" alt="A Flashcards review showing the revealed answer with Again, Hard, Good and Easy rating buttons."></a><br><b><a href="apps/flashcards/README.md">Flashcards</a></b><br>Review flashcards anywhere, even without Wi-Fi.</td>
<td width="33%" valign="top"><a href="apps/frame/README.md"><img width="230" src="docs/media/site/apps/frame.png" alt="Ansel Adams's The Tetons and the Snake River filling the Frame screen on a Kobo Clara BW."></a><br><b><a href="apps/frame/README.md">Frame</a></b><br>Show photos from your computer as a slideshow, with the screen on or waking on a schedule. Needs your computer.</td>
<td width="33%" valign="top"><a href="apps/grimoire/README.md"><img width="230" src="docs/media/site/apps/grimoire.png" alt="Grimoire initiative order showing the active combatant and round counter on a Kobo."></a><br><b><a href="apps/grimoire/README.md">Grimoire</a></b><br>Browse offline SRD references and manage tabletop combat.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="examples/gutenbird/README.md"><img width="230" src="docs/media/site/apps/gutenbird.png" alt="Popular Project Gutenberg books with their covers in Gutenbird on a Kobo"></a><br><b><a href="examples/gutenbird/README.md">Gutenbird</a></b><br>Download free books from Project Gutenberg and other libraries, and read them offline.</td>
<td width="33%" valign="top"><a href="apps/habits/README.md"><img width="230" src="docs/media/site/apps/habits.png" alt="Habits today screen on a Kobo Clara BW, with Read 20 pages checked off and two more habits due."></a><br><b><a href="apps/habits/README.md">Habits</a></b><br>Track daily and weekday habits with local streaks.</td>
<td width="33%" valign="top"><a href="examples/hn/README.md"><img width="230" src="docs/media/site/apps/hackernews.png" alt="A ranked list of Show HN stories on a Kobo"></a><br><b><a href="examples/hn/README.md">Hacker News</a></b><br>Read Top, New, Ask, and Show stories with complete comment threads.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/homepanel/README.md"><img width="230" src="docs/media/site/apps/homepanel.png" alt="Home Panel tile grid on a Kobo showing eight Home Assistant tiles with the last refresh time."></a><br><b><a href="apps/homepanel/README.md">Home Panel</a></b><br>Control saved Home Assistant tiles from a low-power panel. Needs an account. Home Assistant must be on HTTPS.</td>
<td width="33%" valign="top"><a href="apps/inkling/README.md"><img width="230" src="docs/media/site/apps/inkling.png" alt="A solved Inkling puzzle in three guesses, with right-spot letters in black squares."></a><br><b><a href="apps/inkling/README.md">Inkling</a></b><br>Solve a fresh five-letter puzzle each day.</td>
<td width="33%" valign="top"><a href="apps/kitchencard/README.md"><img width="230" src="docs/media/site/apps/kitchencard.png" alt="Kitchen Card showing a large cooking instruction with Steps and Ingredients tabs."></a><br><b><a href="apps/kitchencard/README.md">Kitchen Card</a></b><br>Keep Mealie recipes handy in a counter-friendly cooking view. Needs an account.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/calibre-web/README.md"><img width="230" src="docs/media/site/apps/calibre-web.png" alt="The first page of Pride and Prejudice from a calibre-web library on a Kobo."></a><br><b><a href="apps/calibre-web/README.md">Library</a></b><br>Browse and read books from your calibre-web library. Needs an account.</td>
<td width="33%" valign="top"><a href="apps/lichess/README.md"><img width="230" src="docs/media/site/apps/lichess.png" alt="Lichess on Kobo with Play the computer first, then Play online, Your games, Puzzles and How to play."></a><br><b><a href="apps/lichess/README.md">Lichess</a></b><br>Play Lichess games, challenge players, solve puzzles, or play offline.</td>
<td width="33%" valign="top"><a href="apps/logicpack/README.md"><img width="230" src="docs/media/site/apps/logicpack.png" alt="A solved Hashi puzzle in Logic Pack, with sixteen islands joined by single and double bridges."></a><br><b><a href="apps/logicpack/README.md">Logic Pack</a></b><br>Play four familiar logic games offline.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="examples/magnet/README.md"><img width="230" src="docs/media/site/apps/magnet.png" alt="The Kobo hall sensor responding to a magnet"></a><br><b><a href="examples/magnet/README.md">Magnet</a></b><br>Find the hall sensor behind the bezel and watch it respond to a magnet.</td>
<td width="33%" valign="top"><a href="apps/morse/README.md"><img width="230" src="docs/media/site/apps/morse.png" alt="A letter filling the Kobo screen while the front light sends Morse code"></a><br><b><a href="apps/morse/README.md">Morse</a></b><br>Type a message and send it in Morse code with the front light.</td>
<td width="33%" valign="top"><a href="apps/musicstand/README.md"><img width="230" src="docs/media/site/apps/musicstand.png" alt="Music Stand showing the opening of the Prelude from Bach's Cello Suite No. 1 at reading size."></a><br><b><a href="apps/musicstand/README.md">Music Stand</a></b><br>Read music scores with setlists and half-page turns. Needs your computer.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/needles/README.md"><img width="230" src="docs/media/site/apps/needles.png" alt="Needles row counter showing the current row, section and repeat progress, with +1 row and Undo."></a><br><b><a href="apps/needles/README.md">Needles</a></b><br>Count rows, browse Ravelry collections, and read your synced patterns offline.</td>
<td width="33%" valign="top"><a href="apps/nonograms/README.md"><img width="230" src="docs/media/site/apps/nonograms.png" alt="A Nonograms puzzle with the selected square and its row and column clues highlighted."></a><br><b><a href="apps/nonograms/README.md">Nonograms</a></b><br>Solve 78 picture and pattern puzzles, or make your own from photos.</td>
<td width="33%" valign="top"><a href="apps/panels/README.md"><img width="230" src="docs/media/site/apps/panels.png" alt="The Panels comic shelf with a cover and saved progress, page 3 of 4."></a><br><b><a href="apps/panels/README.md">Panels</a></b><br>Read comics added from your computer or Komga library.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/paperterm/README.md"><img width="230" src="docs/media/site/apps/paperterm.png" alt="Paperterm showing a laptop terminal session in portrait, with the keyboard open."></a><br><b><a href="apps/paperterm/README.md">Paperterm</a></b><br>Pair with a computer to mirror a terminal session on e-ink. Needs your computer.</td>
<td width="33%" valign="top"><a href="apps/parlor/README.md"><img width="230" src="docs/media/site/apps/parlor.png" alt="Reversi against the computer on a Kobo, on a ruled board with the computer's last disc marked and the legal moves shown as grey dots."></a><br><b><a href="apps/parlor/README.md">Parlor</a></b><br>Play Reversi, Draughts, Nine Men's Morris and Kalah on one Kobo.</td>
<td width="33%" valign="top"><a href="apps/parser/README.md"><img width="230" src="docs/media/site/apps/parser.png" alt="Parser opening Lamplight, with every word of the story tappable, a run of verbs such as Look and Take, and a row of compass directions."></a><br><b><a href="apps/parser/README.md">Parser</a></b><br>Play an original interactive story without Wi-Fi.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/post/README.md"><img width="230" src="docs/media/site/apps/post.png" alt="Post inbox listing Hermes letters newest first, with Newer and Older paging."></a><br><b><a href="apps/post/README.md">Post</a></b><br>Read and reply to letters from Hermes. Needs an account.</td>
<td width="33%" valign="top"><a href="apps/arxiv/README.md"><img width="230" src="docs/media/site/apps/arxiv.png" alt="The newest Artificial Intelligence preprints listed newest first in the Preprints app on a Kobo"></a><br><b><a href="apps/arxiv/README.md">Preprints</a></b><br>Browse every arXiv category, keep preprints with their figures, and read them offline on your Kobo.</td>
<td width="33%" valign="top"><a href="apps/pubquiz/README.md"><img width="230" src="docs/media/site/apps/pubquiz.png" alt="Pub Quiz pass-around question on Ada's turn, with four numbered answer choices."></a><br><b><a href="apps/pubquiz/README.md">Pub Quiz</a></b><br>Play offline solo or pass-around trivia rounds.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/readlater/README.md"><img width="230" src="docs/media/site/apps/readlater.png" alt="Read Later's unread queue of synced Wallabag articles with reading times."></a><br><b><a href="apps/readlater/README.md">Read Later</a></b><br>Read your Wallabag articles anywhere. Needs an account.</td>
<td width="33%" valign="top"><a href="apps/readeck/README.md"><img width="230" src="docs/media/site/apps/readeck/inbox.png" alt="Readeck inbox on a Kobo listing four public-domain essays with their reading times."></a><br><b><a href="apps/readeck/README.md">Readeck</a></b><br>Read and search your saved Readeck articles. Needs an account.</td>
<td width="33%" valign="top"><a href="examples/sidekick/README.md"><img width="230" src="docs/media/site/apps/sidekick.png" alt="Sidekick showing several coding-agent sessions and their pending approvals."></a><br><b><a href="examples/sidekick/README.md">Sidekick</a></b><br>Answer coding-agent permission prompts from your Kobo. Needs your computer.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/zotero-reader/README.md"><img width="230" src="docs/media/site/apps/zotero-reader.png" alt="The opening of Darwin's On the Origin of Species, read from a Zotero library's indexed text on a Kobo"></a><br><b><a href="apps/zotero-reader/README.md">Stacks</a></b><br>Browse Zotero collections, read metadata and indexed paper text, and keep papers available offline. Needs an account.</td>
<td width="33%" valign="top"><a href="apps/sudoku/README.md"><img width="230" src="docs/media/site/apps/sudoku.png" alt="An original Sudoku puzzle with pencil notes, an answer entered in grey and the selected square's row, column and box shaded"></a><br><b><a href="apps/sudoku/README.md">Sudoku</a></b><br>Play 600 original Sudoku puzzles with pencil notes, undo and saved games.</td>
<td width="33%" valign="top"><a href="apps/syncthing/README.md"><img width="230" src="docs/media/site/apps/syncthing.png" alt="Sync folders: notes for Vault, photos for Frame and books come from the computer, and out goes back to it."></a><br><b><a href="apps/syncthing/README.md">Sync</a></b><br>Sync folders between your computer and your Kobo with Syncthing. Needs your computer.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="examples/tictactoe/README.md"><img width="230" src="docs/media/site/apps/tictactoe.png" alt="A completed game of tic-tac-toe on a Kobo"></a><br><b><a href="examples/tictactoe/README.md">Tic-tac-toe</a></b><br>Play tic-tac-toe together on one Kobo.</td>
<td width="33%" valign="top"><a href="examples/todo/README.md"><img width="230" src="docs/media/site/apps/todo.png" alt="A to-do list with four open items and one done on a Kobo."></a><br><b><a href="examples/todo/README.md">Todo</a></b><br>Keep a simple to-do list that stays on your Kobo.</td>
<td width="33%" valign="top"><a href="apps/vault/README.md"><img width="230" src="docs/media/site/apps/vault.png" alt="Vault home on a Kobo with eight notes and Browse, Tags, Recent and Search rows."></a><br><b><a href="apps/vault/README.md">Vault</a></b><br>Browse your notes by folder, tag, link, and backlink. Needs your computer.</td>
</tr>
<tr>
<td width="33%" valign="top"><a href="apps/verses/README.md"><img width="230" src="docs/media/site/apps/verses.png" alt="William Blake's The Tiger in Verses, set in a spacious Kobo reading layout."></a><br><b><a href="apps/verses/README.md">Verses</a></b><br>Read a public-domain poem each day.</td>
<td></td>
<td></td>
</tr>
</table>
<!-- store-apps:end -->

Want an app that is not here? Add it to the
[app request thread](https://github.com/BandarLabs/Cobalt/issues/41).

## Features

- Signed app installs, updates and removal over Wi-Fi.
- App pages on the website that install to a linked Kobo by QR code or pairing
  code.
- Every app runs in its own unprivileged process, and must declare the
  services it uses, such as network, storage, audio or the front light.
- Cobalt updates are separate from app updates.
- A declarative e-ink interface toolkit, with full and partial refreshes
  planned for each supported screen.
- A browser simulator for building and testing apps without a device.
- Static ARMv7 binaries, with nothing to install on the reader beyond Cobalt.
- Interrupted installs and updates leave the working version in place.
- Folder sync with your computer through Syncthing.

## How it compares

Menu add-ons put actions in the Kobo's menu, and alternative readers replace
the book reader. Cobalt is a platform for building and installing many kinds
of apps. It handles the screen, touch input, e-ink refreshes, app lifecycle,
device access, isolation and signed installs, so app authors do not have to.
The [FAQ](https://bandarlabs.github.io/Cobalt/faq.html) has more.

## Build an app

```sh
git clone https://github.com/BandarLabs/Cobalt && cd Cobalt
cargo install --path crates/kobo-cli
kobo new my-app
cd my-app
kobo dev
```

`kobo dev` runs the app in the Clara BW simulator in your browser. Set
`KOBO_SIM_PROFILE=libra-2-388` or `KOBO_SIM_PROFILE=elipsa-2e-389` to try the
larger screens, or `KOBO_SIM_PROFILE=clara-colour-393` to see the app as a
colour reader draws it.

| Area | What the SDK provides |
|---|---|
| App model | Plain Rust programs with declarative screens, named actions, lifecycle callbacks and Back navigation |
| E-ink UI | Measured text, rows, tiles, pictures, dialogs, keyboards, terminal views, pagination and refresh planning |
| Network and credentials | HTTPS requests, ranged downloads, size limits, and named secrets that the app never sees |
| State and background work | Per-app storage, cancellable tasks, background and foreground events, and scheduled wake |
| Device and media | Battery, cover sensor, front light, Wi-Fi, Bluetooth and audio, each behind a declared capability |
| Tooling | Scaffolding, simulators, layout checks, failure scenarios, packaging and device deployment |

Apps ask the runtime for services instead of opening devices themselves. A
request can be refused because it was not declared, the battery is too low or
the device does not support it, and the app receives the refusal as a value it
can handle.

Start with the [SDK reference](https://bandarlabs.github.io/Cobalt/sdk.html).
The full guide is [SDK.md](SDK.md), and
[docs/companion-cli.md](docs/companion-cli.md) covers the `kobo` command.

## Contributing

Contributions are welcome: apps, fixes, documentation and device testing. See
[CONTRIBUTING.md](CONTRIBUTING.md). Pull requests go to the `beta` branch.

To add an app:

1. Add it as a workspace package under `apps/<app-id>/`, with its Store
   metadata in `apps/<app-id>/cobalt-app.json`.
2. Add unit tests and layout checks for each supported screen.
3. Run it in the simulators, then on a supported Kobo, and attach a photo,
   GIF or video to the pull request.
4. Include a screenshot for the app's README and website page.

Once merged, the app is signed and published to the Beta catalog. It reaches
Stable when that commit is promoted. See
[docs/CONTRIBUTING_APPS.md](docs/CONTRIBUTING_APPS.md) and
[docs/RELEASE-TRAIN.md](docs/RELEASE-TRAIN.md).

**Own a Kobo that is not tested yet?** You can help without writing code.
[Open or join a device thread](https://github.com/BandarLabs/Cobalt/issues)
with your model, firmware and whether you can run attended tests. See
[device testing](CONTRIBUTING.md#device-testing).

## Repository layout

| Path | Purpose |
|---|---|
| `apps/` | Store apps and the app registry |
| `examples/` | Built-in apps and SDK examples |
| `crates/kobo-sdk` | The application SDK |
| `crates/kobod` | The runtime on the reader |
| `crates/kobo-ui` | Layout and e-ink rendering |
| `crates/kobo-sim` | The simulator |
| `crates/kobo-app-store` | Signed package and catalog formats |
| `crates/kobo-cli` | The `kobo` command: setup, build, simulation, packaging and release |
| `docs/` | Guides, indexed in [docs/README.md](docs/README.md) |

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo run -p kobo-cli -- run --sim --app sudoku
```

See [docs/DEVELOPING.md](docs/DEVELOPING.md),
[docs/DEVICES.md](docs/DEVICES.md) and the [roadmap](ROADMAP.md).

## Safety

Cobalt does not replace the Kobo's boot chain, and a restart always returns to
the stock reader. Installing it adds files to the reader's user storage. It is
provided without warranty. Report security issues as described in
[SECURITY.md](SECURITY.md).

## License

GNU Affero General Public License v3.0. See [LICENSE](LICENSE) and
[THIRD-PARTY.md](THIRD-PARTY.md).

An application links `kobo-sdk`, which is AGPL-3.0-only with no linking
exception, so an application built with it is AGPL-3.0 as well. The SDK crates
are not published to crates.io: Store applications live in this repository and
are built from a checkout of it.

Bluetooth, Wi-Fi and Kobo are trademarks of their owners. See
[TRADEMARKS.md](TRADEMARKS.md).
