# The Kobo application SDK

Write an application for a Kobo e-reader in one file, with no dependencies
outside this workspace, and run it on the panel.

Looking for the available controls and layouts? Jump to the
[UI component reference](#3-ui-component-reference).

```rust
use kobo_sdk::{ActionId, Context, KoboApp, ScreenBuilder};

#[derive(Default)]
struct Hello {
    taps: u32,
}

impl KoboApp for Hello {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == kobo_sdk::action_id("tap") {
            self.taps += 1;
        }
        self.show(context);
    }
}

impl Hello {
    fn show(&self, context: &mut Context) {
        context.set_screen(
            ScreenBuilder::new("hello")
                .top_bar("Hello")
                .heading(format!("{} taps", self.taps))
                .button("tap", "Tap me")
                .build(),
        );
    }
}

fn main() {
    let _ = kobo_sdk::run("hello", Hello::default());
}
```

---

## 0. Your own application, end to end

Before anything else: Cobalt is hardware-tested on the exact Clara BW, Clara
Colour, Elipsa 2E, Clara HD, Libra 2, and Libra Colour identities in the
[device support matrix](docs/DEVICES.md#device-support-matrix). It is
AGPL-3.0 licensed and comes with no warranty. Every device write is gated on an
exact hardware match; do not install it on an unlisted identity or firmware.
To help support another model, start with
[Porting to another Kobo](docs/PORTING.md).

Six steps from nothing to a tile on the reader's launcher.

**1. Make the crate.** There are two starting points, and which you want
depends on where it is going.

To try something quickly, outside the workspace, install the CLI once and use
it from anywhere:

```sh
cargo install --path crates/kobo-cli
kobo new my-app && cd my-app && kobo dev
```

That writes `examples/hello` verbatim: a working application with a screen,
two buttons, a battery reading and two passing tests. It runs in the simulator
immediately. It cannot be packaged or given a launcher tile, because it is not
a workspace member. (`cargo run -p kobo-cli` only works from inside this
repository, which is why the CLI goes on your PATH for this route.)

For something you intend to put on a reader, start in `examples/` instead.
Copy the smallest application as a base:

```sh
cp -r examples/todo examples/myapp
```

Its `Cargo.toml` is the whole manifest an application needs:

```toml
[package]
name = "kobo-myapp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[dependencies]
kobo-sdk = { path = "../../crates/kobo-sdk" }

[lints]
workspace = true
```

**2. Join the workspace.** Add `"examples/myapp"` to `members` in the root
`Cargo.toml`.

**3. Write it.** One `impl KoboApp`, as at the top of this file. The name you
pass to `kobo_sdk::run` is the application's identity: its store namespace, its
data directory and the name the launcher starts.

**4. See it.** Nothing here needs a device:

```sh
cargo run -p kobo-cli -- dev --builtin          # in a browser
cargo run -p kobo-cli -- run --sim --app myapp  # against the real runtime
```

The simulator uses the same layout engine, the same typeface and the same
refresh planner the panel uses. Its diagnostics panel reports content past the
fold, clipping, undersized targets and text overflow, which is where most
first-draft screens fail.

**5. Choose how it ships.** The quickstart above creates a bundled example.
Register one only when it is a platform-owned system app:

- `INSTALLED_PACKAGES` in `crates/kobo-cli/src/main.rs`, so `package` builds it.
- `ENTRIES` in `examples/launcher/src/main.rs`, so it gets a tile. The entry
  states what starting it costs the device, because a launcher that starts
  something without saying what it reaches for is asking the owner to find out
  afterwards.

For a contributed app that owners install on demand, put it under `apps/` and
follow [the Store contribution guide](docs/CONTRIBUTING_APPS.md). Register it in
`STORE_PACKAGES` and `apps/catalog.json`; do not add it to `INSTALLED_PACKAGES`
or the launcher's built-in entries.

**6. Put it on the reader.** With SSH already working, no cable and no reboot:

```sh
cargo run -p kobo-cli -- devices                     # find the address
cargo run -p kobo-cli -- deploy --device <address>
```

Without SSH, or for somebody else's device:

```sh
cargo run -p kobo-cli -- package     # writes target/KoboRoot.tgz
```

Copy that to `.kobo/KoboRoot.tgz` on the reader over USB and eject. The reader
installs it at the next boot. Everything lands in `.adds/cobalt` on the book
partition; uninstalling is deleting that folder.

Install the Rust target and an ARM hard-float C compiler (the
`gcc-arm-linux-gnueabihf` package on Debian). Set
`CC_armv7_unknown_linux_musleabihf=arm-linux-gnueabihf-gcc` when building.
That compiler builds the maintained `ring` provider; the resulting program is
still statically linked. Section 11 has the rest of the device story, section
12 has the rules the SDK will not let you break.

---

## 1. The shape of an application

An application is a process. It connects to the runtime over a Unix socket,
sends whole screens, and receives actions. It never opens the framebuffer,
never touches the input device, never opens a socket to the internet, and never
sees a credential.

```
your binary ── kobo-sdk ──socket── kobod ── panel, touch, network, secrets
```

Everything an application cannot do itself, it asks for. Everything it asks for
can be refused, and a refusal is a value it must handle rather than a crash.

### Declarative, not retained

You do not mutate widgets. On every event you describe the screen you want and
hand it over:

```rust
context.set_screen(ScreenBuilder::new("results").heading("Results").build());
```

### Portrait and landscape

Portrait is the compatibility default. An application that genuinely benefits
from a wide viewport may request landscape for its current app session:

```rust
context.set_orientation(kobo_sdk::Orientation::Landscape);
let metrics = context
    .metrics()
    .oriented(kobo_sdk::Orientation::Landscape);
```

The runtime keeps the framebuffer in its verified native mode and rotates the
rendered surface in software when a hardware backend has not been independently
validated for quarter turns. Touch coordinates are transformed through the
same mapping. On readers that report a verified physical landscape side, the
runtime follows that side; readers without orientation events use a fixed
clockwise convention. Returning to the reader or starting another application
restores the portrait default automatically.

The runtime diffs it against the last one and picks an E Ink waveform from the
pixels that changed. This is not a stylistic preference. A retained tree
invites incremental mutation, incremental mutation on E Ink means many small
partial refreshes, and many small partial refreshes on this hardware means
visible ghosting.

### Actions are names

```rust
.button("search", "Search the library")
```

`"search"` is hashed into a stable [`ActionId`]. The same name always produces
the same identifier, so `on_action` can compare against `action_id("search")`
without threading indices through your state. Two different labels may share an
action; that is deliberate, and it is how a control appears in both a nav bar
and a button.

---

## 2. `KoboApp`

```rust
pub trait KoboApp {
    fn on_start(&mut self, context: &mut Context);
    fn on_action(&mut self, context: &mut Context, action: ActionId);

    fn on_resume(&mut self, context: &mut Context) {}
    fn on_suspend(&mut self, context: &mut Context) {}
    fn on_scheduled_wake(&mut self, context: &mut Context) {}
    fn on_exit(&mut self, context: &mut Context) {}
    fn on_device_result(&mut self, cx: &mut Context, request: DeviceRequest, result: DeviceResult) {}
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {}
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {}
    fn on_shell_event(&mut self, context: &mut Context, event: ShellEvent) {}
    fn on_background(&mut self, context: &mut Context) {}
    fn on_foreground(&mut self, context: &mut Context) {}
}
```

Only the first two are required. Every callback is handed a `&mut Context`,
which is the only way to affect the outside world; a method that does not take
one cannot.

`fn main` is `kobo_sdk::run("name", app)`, which reads the socket path from
`KOBO_SOCKET`. Use `run_on` to name a socket yourself.

---

## 3. UI component reference

Apps build screens from the components below through `ScreenBuilder`. The
builder is a chain: every method that adds or configures UI returns `Self`.
This is the app-facing UI vocabulary; apps do not draw arbitrary pixels or
instantiate renderer-internal `Node` variants directly.

The [component gallery](examples/gallery/README.md) shows these primitives on
the actual E Ink renderer and is the quickest visual reference.

### Screen structure and overlays

| Method | What it is |
|---|---|
| `top_bar(title)` | The fixed bar at the top. Back is added by the runtime. |
| `top_bar_action(name, label)` | One trailing control in the top bar. |
| `top_bar_glyph(name, label, glyph)` | A trailing top-bar control drawn with a built-in glyph. |
| `owns_back(bool)` | Ask for Back as an action before it leaves the app. |
| `nav_bar(selected, [(name, label), ...])` | The pinned bottom bar of *destinations*. At least two. |
| `action_bar([(name, label), ...])` | The same slot, carrying *verbs* instead. At most three. |
| `action_bar_marked([(name, label, glyph), ...])` | The same, with an optional mark drawn above each word. |
| `bottom_action(name, label)` | One pinned full-width control in the bottom band. |
| `bottom_action_marked(name, label, glyph)` | One pinned control with its mark beside the word. |
| `top_bar_overflow(name, open, [(name, label), ...])` | Three dots that open a menu when `open`, and close on a tap anywhere else. |
| `popover(anchor, build)` | An anchored overlay that dismisses when the reader taps outside it. |
| `modal(title, build)` | A centred overlay that remains until one of its controls answers it. |
| `page_turns(previous, next)` | Tap the left of the page to go back, the rest to go on. |
| `page_position(page, total)` | A footer line saying where in the list this is. |
| `reading_menu(name)` | Adds a middle page-turn zone for a reader's controls. |
| `hold(name)` | Sends an action when the content area is held. |
| `tabs(selected, [(name, label), ...])` | A row of tabs over one region of the screen. |

### Text and document content

| Method | What it is |
|---|---|
| `heading(text)` | One line of display type. |
| `heading_at_level(level, text)` | A heading in a book or document hierarchy. |
| `text(text)` | A paragraph. Wraps by measured glyph width and Unicode line-break rules. |
| `text_linking(text, links)` | A paragraph with tappable byte ranges. |
| `rich_text(text, spans, presentation)` | Publisher-styled prose with semantic spans and paragraph presentation. |
| `rich_text_linking(...)` | Rich text with tappable inline destinations. |
| `selectable_rich_text_linking(...)` | Rich text whose words can be resolved when held. |
| `with_formulae(formulae)` | Adds prepared inline formula pictures to the rich-text paragraph just added. |
| `secondary(text)` | Muted metadata such as an author, date, size or status. |
| `section(title)` / `section_with_value(title, value)` | A labelled group, optionally with a count or total. |
| `facts([(label, value), ...])` | Labelled values with one shared, measured label column. |
| `quote(depth, text)` / `byline(depth, text)` | Thread body text and its smaller, muted author line. |
| `folding_byline(...)` | A tappable byline that represents a collapsed reply subtree. |
| `table(rows, weights)` | Tabular document content with columns measured across all rows. |
| `picture(picture, max_height_mm)` | A framed picture fitted to a physical height. |
| `unframed_picture(picture, max_height_mm)` | The same without a border, for formulas and in-flow document art. |

### Actions and input

| Method | What it is |
|---|---|
| `button(name, label)` | A full-width action. |
| `primary_button(name, label)` | The single filled, primary action on a screen. |
| `buttons([(name, label), ...])` | Two or three secondary actions on one line. |
| `disabled_button(name, label)` | A visible, outlined action that yields nothing and absorbs its tap. |
| `button_with_state(name, label, state)` | A button with explicit semantic enabled state. |
| `field(name, value, placeholder)` | A tappable field showing its current value; the app routes it to a keyboard screen. |
| `field_clear(name)` | Adds a clear control to the field just declared when it has a value. |
| `chips([(name, label, selected), ...])` | Wrapping filters, tags or recent searches with selected state. |
| `choose(prompt, [(name, label), ...])` | A question with tappable answers. |
| `chosen(index)` | Marks which answer of the preceding `choose` is already given. |
| `or_type(name, placeholder)` | A freeform row on the end of a `choose`. |
| `stepper(label, less, less_glyph, more, more_glyph)` | A setting that moves one notch at a time. |
| `stepper_ends(less, more)` / `stepper_track(percent)` | Configures enabled ends and the optional position track of the preceding stepper. |
| `keyboard(&keyboard, submit)` | The on-screen text keyboard, pinned under the thumbs. |
| `text_entry(&entry, prompt, submit)` | A complete prompt, typed value, keyboard and cancel action. |
| `typed(&keyboard, placeholder)` | The text accumulated by a keyboard, or its placeholder. |

### Collections, grids and media

| Method | What it is |
|---|---|
| `rows([(name, title, summary, glyph), ...])` | A list. Title, one line of detail, an icon. |
| `checklist([(name, title, summary, done), ...])` | The same list, where a finished row is struck through. |
| `rows_with_menu([(name, title, summary, glyph, menu), ...])` | The same list, where each row carries an overflow mark naming an action of its own. |
| `rows_with_trailing([(name, title, summary, lead, value), ...])` | Rows with a short score, date, size or count at the trailing edge. |
| `row_overflow(anchor, open, [(name, label, glyph), ...])` | The menu behind that mark. |
| `paged_list(page, items)` | A pre-paged list of plain strings. |
| `tiles([(name, label, glyph), ...])` | A grid of square destinations. |
| `apps([metadata, ...])` | Launcher tiles from `AppMetadata`, with picture-to-glyph fallback. |
| `grid(columns, square, cells)` | A general grid of labelled buttons, optionally square. |
| `board(columns, cells)` | A square board whose filled cells are large built-in marks. |
| `controls(columns, [(name, label, glyph), ...])` | Compact glyph-and-label controls for universally understood actions. |
| `picture_tiles(shape, [...])` | A grid of tiles that each carry a picture, falling back to a glyph. |
| `tile_grid(shape, \|tile\| ...)` | Configurable tiles with subtitles, badges, pictures and semantic state. |
| `hero(picture, mm, title, ...)` | A picture beside a column of title, metadata and facts. |

### Feedback, loading and states

| Method | What it is |
|---|---|
| `banner(level, text)` | `Info` or `Attention`. Attention is drawn inverted. |
| `progress(percent)` | A determinate bar. |
| `activity(label, progress)` | Work in flight, indeterminate with `None` or coarse determinate progress with `Some(percent)`. |
| `cancellable(name, label)` | Adds a cancel control to the preceding activity. |
| `skeleton(lines)` | Placeholder lines, occupying where content will land. |
| `splash(glyph, title, summary)` | A mark, a title and a sentence, centred in the room that is left. |
| `empty_state(message)` | A standard empty result with a useful default title. |
| `offline_state(message)` | A standard offline presentation; chain a retry button when useful. |
| `permission_denied_state(message)` | A standard denied-capability presentation. |
| `error_state(message)` | A standard recoverable-error presentation. |
| `failure_state(failure, retry)` | Maps a task failure to a standard state and the valid recovery actions. |
| `confirmation(title, message, primary, secondary)` | A whole-screen confirmation with two `DialogAction`s. |
| `confirm(title, question, confirm, cancel)` | A modal confirmation with standard primary and cancel controls. |
| `transfer(label, received, total)` | Byte transfer progress, determinate only when a total is known. |
| `transfer_failed(...)` / `transfer_retry(...)` | A transfer that stopped, and the way back into it. |

### Layout and specialised surfaces

| Method | What it is |
|---|---|
| `divider()` / `spacer(space)` | A rule and semantic space from the design scale. |
| `fill()` | Pushes everything after it to the foot of the panel. |
| `band(align, [slot, ...])` | Up to three blocks side by side, stacking automatically when too narrow. |
| `compose(build)` | Reuses a function that builds a group of components without breaking the chain. |
| `section_rows(title, value, rows)` | A section, optional trailing value and its rows in one call. |
| `terminal(rows, cursor)` | A fixed character grid with an optional block caret. |
| `terminal_keys(&keys)` | Terminal keys that send bytes immediately, including control and cursor keys. |
| `text_scale(scale)` / `reading(reading)` / `reading_font(font)` | Reader-only typography configuration; ordinary app UI follows the user's scale. |
| `build_checked()` | Builds only when screen diagnostics contain no errors. |

There is no free-form drawing, no colour, no font choice and no pixel
positioning. Every size comes from the panel's *physical* dimensions, so a
control that is comfortable under a thumb on a six inch panel is comfortable on
a ten inch one, and a line of text holds roughly the same number of words on
both.

Wrapping uses the installed face's measured glyph advances, Unicode line-break
opportunities and grapheme boundaries. It does not estimate from character
count, split a combining sequence, or assume spaces are the only place a line
can break. Set `KOBO_TEXT_SCALE=large` or
`KOBO_TEXT_SCALE=extra-large` to run the simulator and runtime at 120% or 140%.
The selected scale is part of the runtime handshake, so application pagination
and device rendering use the same metrics.

### Icons

`Glyph` is a closed set. `Glyph::ALL` is the whole of it and is what the
gallery and the vector tests enumerate, so a glyph that is added without being
drawn fails a test rather than shipping as an empty box.

They are geometry, not bitmaps: authored in a 1000 unit box and rasterised
with coverage antialiasing at whatever size the layout asks for, so they are
crisp at every density. Applications cannot supply their own paths: arbitrary
path data is untrusted input to a rasteriser, and an application must not be
able to draw something indistinguishable from a system control.

The artwork is [Tabler Icons](https://tabler.io/icons), which is MIT, converted
once into checked-in Rust at `crates/kobo-ui/src/vector/tabler.rs`. Nothing at
build time or run time reads an SVG or reaches the network, so the workspace
keeps its no-dependency rule. It is a published set because forty
hand-drawn ones are forty separate judgements
about how round a corner runs and how long a tail is, and a set drawn that way
looks like a set only from across the room.

Adding one is two lines and a command. Name the `Glyph`, put it in `Glyph::ALL`,
add a row to `tools/icon-import/icons.txt` naming any of the five thousand
icons in the upstream set, and run `scripts/import-icons.sh`. The lookup the
importer generates is exhaustive, so a `Glyph` with no row in that file does
not compile. Then draw it in the gallery, or the conformance test fails. Judge
the result by eye rather than by test:

```
cargo test -p kobo-ui contact_sheet -- --ignored --nocapture
```

draws every glyph onto one sheet and says where it put it. The wire tag for a
glyph is one byte, so the set can hold 256 and no more, which is why it
is curated.

Three places take one: `rows` and `tiles`, where the icon leads a title, and
`controls`, where it replaces one. There is deliberately no way to put a glyph
on `button`. A full-width button already has room to say what it does.

A `controls` button draws the picture and nothing else. Setting both is the
worst of the two, because an icon that has to be checked against a word
underneath it is slower to read than either on its own.

That is a high bar, so reach for `controls` only when the picture is genuinely
universal. Play, pause and skip are drawn the same way on every device anyone
has used. "Create another" has no such picture, and a shape invented for it is
a shape nobody can read.

It also means the glyph has to carry everything the word did. An arrow says
"back" but cannot say how far, which is why the skip glyphs are `Rewind30` and
`Forward30` with the numeral drawn inside the arc rather than a plain arrow
next to the words "30 sec". If you cannot draw the whole meaning, use a label.

The label is still given and still carried on the wire. It is the action's
name and the only thing a reader could be told out loud; it is simply not set
on the panel. Anything that must be *read* belongs somewhere that is read: the
audiobook player moved "Loading…" off its play button and onto the position
line above it, which was going to change anyway.

### How a screen is composed

The vocabulary above is deliberately small, and a small vocabulary only reads
as a product if it is spent the same way every time. These are the rules the
nine shipped applications follow. Some of them the renderer now enforces as
diagnostics; the rest are here because a screen that breaks them looks wrong
without anybody being able to say why.

**One screen has one heading.** A second heading is a second screen that has
not been separated yet. Below it, at most one `primary_button`, the thing the
reader came here to do, and at most one region of supporting detail. If two
actions are equally important, neither is primary and both are ordinary
buttons.

**Two secondary actions go side by side, not stacked.** Use `buttons`. Stacked,
each one takes the full width of the panel to say a single word and the bottom
of the screen reads as a form. `buttons` is `band` underneath, so a narrow
panel still stacks them by itself rather than squeezing three words into a
third of a screen each. Past three, it is a menu: use `row_overflow`.

**Chrome belongs to the runtime.** The top bar carries the title, Back and at
most two trailing controls; anything further goes behind
`top_bar_overflow`. The bottom slot carries **destinations or verbs, never
both**: `nav_bar` answers "where am I", `action_bar` answers "what can I do
here", and a bar that mixes them leaves a reader unable to predict what a tap
costs. That conflation is why `nav_bar(None, …)` used to be written; it is a
warning now, and `action_bar` is the answer.

**A keyboard belongs at the foot of the panel.** `keyboard` puts a `fill` in
front of itself, so the keys are under the thumbs wherever they were added and
whatever is above them. Reach for `fill` directly for anything else that has to
sit on the bottom edge with content above it; it only ever pushes down, so a
screen that is already full is laid out exactly as it was.

**A bar entry keeps its word.** `action_bar_marked` and `bottom_action_marked`
draw the mark above or beside the label, never instead of it. A bar slot is a
third of the panel wide, so "Return to Kobo reader" set across one is a
sentence where a chevron or a house would do; but this band is often the only
way off a screen, which makes it the last place to make somebody guess. The
top bar is the one place a mark does replace the word, because it has no room
for both.

**A screen has exactly one bottom band.** `nav_bar`, `action_bar` and
`bottom_action` all claim it, and the last one called silently wins. A screen
that both navigates and acts is a screen that wants to be two: put the verbs
on the pushed screen, which has `owns_back(true)` and no destinations to
carry. Calling two of them is reported as a layout warning rather than
swallowed.

**A state screen is a splash, not a heading.** `empty_state`, `offline_state`,
`permission_denied_state` and `error_state` all set a mark, a title and a
sentence centred in the content area, because six words ranged left at the top
of a 1448-pixel panel read as a page that failed to load. The splash stops
short of whatever is chained after it, so a recovery `button` still lands
underneath. Reach for `splash` directly when the default title is not the
sentence you want.

**Three type levels per screen.** Display, body, caption. A fourth is
invariably a heading being used for emphasis, which is what a section is for.

**No screen uses more than four of the five inks.** The tones are ink, muted,
surface, hairline and inverted. Using all five on one panel means two of them
are separated by less contrast than E Ink resolves, and the distinction the
fifth was carrying is simply not visible.

**Group before you separate.** A `section` is almost always the right answer
where a `spacer` and a `divider` were reached for; a run of spacers is the
clearest possible evidence that a labelled group was missing.

**Never break a section header from its first row.** `paginate_rows_in_sections`
exists for this. A header alone at the foot of a page with its contents
overleaf is the most common way a paginated layout reads as broken, and on a
panel that takes a second to turn, the reader has a whole second to look at it.

**One way out.** The runtime's Back plus `owns_back` is already the way back.
A screen that also offers "Back to the results" has three controls
meaning one thing and no way for the reader to tell which is which.

### The words

The type is set for you. The words are not, and they are most of what a screen
communicates.

**Buttons take verbs.** "Read", "Download", "Retry". A button labelled with a
noun is a label with a border around it.

**Never two controls with the same word on one screen.** The shelf used to
carry "Back" in the page-turn band meaning *previous page* and "Back" in the
nav bar meaning *leave the shelf*.

**A refusal names the cause and the remedy.** "No network" is a symptom;
"Wi-Fi is off, turn it on from the top bar" is an answer. `offline_state`,
`permission_denied_state` and `error_state` exist so the shape is consistent;
the sentence inside is still yours.

**Never invent a number.** If the source does not supply a total, the transfer
shows bytes received and no percentage. If it does not supply a date, there is
no date. A fabricated fact is the one failure a reader cannot detect and the
one this SDK will not help you with. See *Refusing rather than inventing*.

**State is a field, not a suffix.** `TileState::Held`, not `format!("{title}
(kept)")`. See the next section, which is the same argument at the renderer's
level.

### State is carried, never drawn into a label

A finished row, a chosen answer and a reply's depth are all *state*: the
application says what is true and the renderer decides what it looks like.
There is no way to ask for a line through text, a tick beside a label or an
indent, and that is deliberate. An application that marks its own choice with
a character picks one the installed face may not have, and gets an empty box on
the panel. In debug builds `set_screen` refuses a screen carrying a character
the face cannot draw, so an application's own tests fail rather than the panel.

A disabled button is state as well. Use `disabled_button` or
`button_with_state`; the renderer gives it an outlined, muted treatment and the
layout engine returns no action for it. It still consumes the tap that lands on
it, so a greyed-out control on a paginated screen cannot turn the page instead
of doing nothing.

### Navigation, confirmations and standard states

Applications still own their destinations, but they no longer need to
hand-roll a fallible `Vec` stack:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
enum Route { Home, Details(u32), ConfirmDelete(u32) }

let mut nav = kobo_sdk::Navigator::new(Route::Home);
nav.push(Route::Details(42));
nav.replace(Route::ConfirmDelete(42));
assert!(nav.back());
```

The root can never be popped and `current()` always returns a route.

To make the runtime's Back control drive that stack, ask for first refusal on
it. Pass `can_go_back()` and the behaviour follows the stack for free. Deep
screens pop, the root leaves the application:

```rust
ScreenBuilder::new("book")
    .top_bar(title)
    .owns_back(nav.can_go_back())
    .build()
```

```rust
fn on_action(&mut self, context: &mut Context, action: ActionId) {
    if action == ActionId::BACK {
        // Only ever delivered on a screen that asked for it.
        self.nav.back();
        self.show(context);
        return;
    }
    // ...
}
```

**This is first refusal, not ownership.** The Back control is still the
runtime's, still drawn by the runtime, and still leads out of the application
in the end. An application that claims it and then draws nothing in answer is
left behind and the launcher appears anyway, after about two seconds. There is
no way to hold the reader, which is the whole reason the affordance is
trustworthy enough to be the way out of anything.

Without this, Back always went straight to the launcher, so a reader who
tapped out of a book landed at home instead of the shelf, and reopening the
application showed the book again, because its retained screen had never
changed.

Confirmations are deliberately whole screens, not floating desktop windows:

```rust
ScreenBuilder::new("delete-note")
    .confirmation(
        "Delete this note?",
        "This cannot be undone.",
        kobo_sdk::DialogAction::new("delete", "Delete"),
        kobo_sdk::DialogAction::new("cancel", "Cancel"),
    )
    .build()
```

For catalogue entries, declare one compile-time `AppMetadata` value. Its
`AppIcon` may be a built-in vector glyph or a prepared square picture with a
glyph fallback, so a missing or evicted bitmap never leaves a blank tile.

```rust
const APP: kobo_sdk::AppMetadata = kobo_sdk::AppMetadata::new(
    "notes",
    "Notes",
    "Write without distraction.",
    kobo_sdk::AppIcon::glyph(kobo_sdk::Glyph::Note),
);

let launcher = ScreenBuilder::new("apps").apps([APP]).build();
```

### Threaded replies

```rust
for (depth, paragraph) in &page {
    screen = screen.quote(*depth, paragraph);
}
```

`quote` sets a paragraph in by one step per level, up to `MAX_QUOTE_DEPTH`,
with a rule down the gutter. Paginate the same shape with
`context.paginate_quoted(&paragraphs, nav_bar)`. An indented paragraph is
narrower, wraps to more lines and eats more of the page, so a thread paginated
flat and drawn indented loses the bottom of nearly every page.

### Clamped labels

`context.clamped_row(text, lines, nav_bar)` measures against the real installed
face and ellipsises, so a row is never taller than you allowed for.
`one_line_row(text, nav_bar)` is the same thing with `lines` of 1.

One line gives a list of uniform rows and is right for labels you wrote
yourself. For text written elsewhere (a headline, a subject line, a filename)
use two: one line ellipsises most real headlines mid-sentence, and
`paginate_rows` measures every row separately, so rows of different heights
cost nothing but the ragged edge.

Measure with the call that matches the row you draw. A row built with
`rows_with_trailing` gives up text width to the value at its trailing edge, so
the full-width helpers measure it at a width it will never have: `clamped_row`
lets a two-line title spill onto a third, and `paginate_rows` packs one row too
many and the last one is drawn under the bottom bar. The trailing-aware pair is
`clamped_row_beside(text, trailing, lines, nav_bar)` and
`paginate_rows_with_trailing(&[(title, summary, trailing), …], nav_bar)`. A row
built with `rows_with_menu` gives up a whole touch target to the mark, whatever
its title says, and its pair is `one_line_row_with_menu(text, nav_bar)` and
`paginate_rows_with_menu(&[(title, summary), …], nav_bar)`.

Two other things a page's measure has to be told, because both cost whole rows
rather than a few pixels:

- **Where the page position goes.** A screen that pages gets a strip under the
  list for "3 of 10", and the layout engine reserves it before it places
  anything. It reserves nothing when there is nothing to draw there, so a
  screen that has already said which page it is on in its top bar wants
  `paginate_rows_with_trailing_at(rows, nav_bar, Position::Elsewhere)` and gets
  the row that strip was holding. The default is `Position::AtTheFoot`.
- **What the rows lead with.** The lead column is as wide as what sits in it,
  and a rank is narrower than a mark. A ranked list measured as a marked one
  hands every title less width than it is drawn with, wraps headlines that
  would have fitted, and comes back short:
  `paginate_ranked_rows_with_trailing(rows, nav_bar, highest, position)`, where
  `highest` is the largest rank the list will show.

### A second thing to do to a row

A row has one obvious verb: open it. Everything else a reader might want to do
to the thing it names -- stop following it, forget it, rename it -- has nowhere
to live on a panel with no long press worth relying on and no room for a second
button beside every entry.

`rows_with_menu` puts a vertical three dot mark against the right edge of each
row, naming an action of its own. It is hit-tested ahead of the row, so a tap
on the dots is never also a tap on the row, and it is inverted on its own while
it is held rather than lighting the whole row. An empty menu name means no mark
on that row.

`row_overflow(anchor, open, items)` hangs the menu off the mark, and is the
same shape as `top_bar_overflow`: what is open is the application's state, not
something the builder remembers. Two things to get right:

- Pass `open` as false when the row is not on the page being drawn. A popover
  anchored to a control that is not on the panel has nothing to point at.
- A tap beside a popover arrives as `ActionId::BACK`. On a screen that would
  otherwise let the runtime take Back, claim it with `with_own_back(true)`
  while the menu is open, or putting the menu away closes the application.

Feeds is the worked example: the mark on a feed offers to stop following it,
which used to mean opening the feed first and fetching a feed you had already
decided you did not want.

### What stands at the head of a row

The fourth element of a `rows` tuple is anything that converts into a
`RowLead`: a `Glyph` for an icon, or a `u16` for a position.

An icon makes a row findable without reading it, which is the point of the
well. But an icon that is the same on every row has spent a whole touch
target's width saying nothing. A list of stories does not need a newspaper
beside each entry to explain that it is a list of stories. Where the entries
are ordered, pass the position instead:

```rust
screen.rows(stories.iter().enumerate().map(|(index, story)| (
    format!("story-{index}"),
    context.clamped_row(&story.title, 2, true),
    story.summary.clone(),
    u16::try_from(index + 1).unwrap_or(u16::MAX),
)))
```

Numbers are set in the same square an icon would have occupied, so a list that
numbers some rows and illustrates others still lines up down its left edge.

### Pictures

`kobo-image` decodes JPEG and PNG on the host and on the device. Decode applies
JPEG EXIF orientation and composites transparent PNG pixels onto the panel's
white paper rather than black. `prepare(width, height, mode)` makes the crop
policy explicit: `FitMode::Contain`, `ContainEnlarging`, or centre-cropped
`Cover`. Every allocation is checked against the decoded-pixel limit before it
is made. `dither(PANEL_GREYS)` then reduces the result to the sixteen greys the
panel resolves using two scanline error buffers rather than another full-image
allocation.

Pass the prepared pixels to `context.put_picture`. The SDK sends small images
inline and automatically streams large or full-panel images in bounded chunks.
The runtime publishes a picture to the cache only after the final complete,
ordered chunk commits, so a partially transferred photograph can never flash
onto the panel. The application API is the same at either size.

---

## 4. Pagination, and why there is no scrolling

There is no scroll view and there should not be. A panel that takes the better
part of a second to repaint cannot follow a finger, and a partial refresh
chasing a moving list is precisely the operation that leaves ghosting behind.

The layout engine stops placing nodes once the cursor passes the bottom of the
content area. Anything that must stay reachable therefore belongs in the nav
bar, which is reserved before content is placed, never at the end of the flow,
where it is the first thing to be dropped. This failure is observable:

```rust
let screen = builder.build_checked()?;            // collection truncation
let issues = screen.validate(&context.metrics()); // overflow, clipping, text,
                                                   // touch targets and glyphs
```

`validate` measures with the back chrome the runtime gives every application,
which is the smaller content area, so a screen that passes here is not clipped
once the runtime adds it.

`Screen::diagnostics` returns the measured layout and its issues from one pass.
The browser simulator lists those issues beside the panel and can outline their
rectangles, including missing pictures held only by the runtime cache.

Ask the runtime where the folds are:

```rust
let pages: Vec<Vec<String>> = context.paginate(&book_text, /* nav_bar */ true);
let pages: Vec<Vec<usize>> = context.paginate_rows(&rows, true);
```

Both measure with the same wrapping, line height and spacing the layout engine
uses, against the panel the runtime actually named. A page that fits here is a
page that will be drawn whole.

A character budget cannot do this. A page of description holds noticeably more
text than a page of dialogue, because dialogue is mostly short paragraphs and
the gaps between them.

---

## 5. Work that takes time

Never block in a callback. The event loop is the thing drawing the screen.

```rust
let task = context.spawn(Task::Fetch {
    url: "https://gutendex.com/books?search=austen".into(),
    offset: 0,
    max_bytes: 64 * 1024,
});
```

`spawn` returns immediately with an `Option<TaskId>`, `None` if the runtime
would not even queue it, and the result arrives at `on_task`:

```rust
fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
    match outcome {
        TaskOutcome::Completed(bytes) => { /* … */ }
        TaskOutcome::Failed(error)    => { /* Denied, Unreachable, TooLarge,
                                             TimedOut, NotFound */ }
        TaskOutcome::Cancelled        => { /* you asked */ }
    }
    self.show(context);
}
```

The four kinds of work:

- **`Fetch { url, offset, max_bytes }`**. HTTPS only. `offset` reads a long
  document in pieces; a range is sent for every piece including the first.
- **`Post { url, body, content_type, secret, max_bytes }`**. `secret` is the
  *name* of a credential the runtime holds. Never its value.
- **`ReadFile { path }`**. Confined to the application's own directory.
- **`Sleep { seconds }`**. Waits without holding a wake lock.

Show that something is happening. `activity(label, None)` plus `skeleton(n)`
puts a placeholder where the content will land, which reads far better on a
slow panel than an empty screen that suddenly fills.

### What the runtime does with a request

The application says what it wants. Everything below is the runtime's, and
none of it can be set from a manifest or a header, because getting any of it
wrong is a way to be slow or to be unsafe.

- **Replies arrive compressed.** The runtime asks for gzip and expands what
  comes back, so a feed search that is 153 KB of JSON crosses the radio as
  15 KB. `max_bytes` is measured against the **expanded** size, which is the
  size the application has to hold, and is also what stops a small reply
  expanding into a large one.
- **A ranged read is the exception.** `offset` names bytes as the server
  stores them, and a window into a compressed stream cannot be expanded on its
  own, so a piece of a document is asked for uncompressed. The bytes counted
  are the bytes asked for.
- **The connection is kept.** A screen of Hacker News comments is two dozen
  requests to one host, and each used to pay for its own TCP connection and
  TLS handshake. The runtime holds the last connection open and asks the next
  question on it, which on a slow link roughly halves the time to fill that
  screen. Only a `Fetch` does this: a `Post` may have been acted on by the far
  end, so it is never the request that gets repeated.

None of this changes what `on_task` receives. It is the same bytes, sooner.

### Work that takes minutes

A bar that sits at thirty percent for ninety seconds is indistinguishable from
a hung application, and nothing on this panel animates to say otherwise. For
anything longer than a few seconds, run a `Heartbeat` alongside the work and
count up on the screen:

```rust
self.clock = Heartbeat::every(5);
self.clock.start(context);

// First in on_task, and returned from immediately, or a nap is mistaken
// for the provider's reply.
if self.clock.on_task(context, task) {
    context.set_screen(self.screen());   // "2 min 45 s so far"
    return;
}
```

`Heartbeat::default()` is five seconds, not zero. Stop it on every path that
ends the work, including cancel and failure, or it naps forever.

A `Post` body may be up to `MAX_POST_BODY_LEN`, which is far larger than the
16 KiB ceiling on a label, because a request that carries research or a
document is not a string on a screen. `Context::spawn` returns `None` rather
than failing the process if a task is still too large to send, so an
over-large request is something the application shows rather than something
the runtime dies of.

### Credentials

```rust
Task::Post { secret: Some("openai".into()), .. }
```

```rust
Task::Post { credential: Some(Credential::in_header("anthropic", "x-api-key")), .. }
```

The application names a secret; the runtime reads
`/mnt/onboard/.adds/cobalt/secrets/<name>` and attaches it, either as a bearer
token or under the header the application named, which is what lets a request
go straight to Anthropic or Gemini rather than through a proxy.
The value is never in the application's memory, its logs, or its crash dump,
and it cannot be sent anywhere the application did not name: the request is
not replayed across a redirect.

`Failure::of(error)` turns a task failure into a state, a sentence and an
honest answer about whether a Retry control would help. For a missing
credential, `Failure::naming(secret)` says which one: an application running
against three providers that only says "install a key" leaves whoever is
holding the reader to guess which of the three.

Getting a key onto the reader is a command, not an errand:

```sh
kobo secret set openai --from ~/.openai --device 192.168.1.5
kobo secret set exa --from ~/.exa --device 192.168.1.5
kobo secret set elevenlabs --from ~/.elevenlabs --device 192.168.1.5
kobo secret list --device 192.168.1.5      # names only, never values
kobo secret remove openai --device 192.168.1.5
```

With no `--from`, the key is looked for in `$KOBO_SECRETS_DIR/<name>`,
`~/.config/cobalt/secrets/<name>` and `~/.<name>`, in that order. The value is
read on this machine and written straight to the reader: it is never passed as
an argument, so it does not reach a process table or a shell history, and it is
never printed. A one-line `NAME=value` file is accepted as well as a raw key;
only the value is installed. `--volume` does the same thing over USB for a
reader that is not yet on Wi-Fi.

### Owner trust roots

Every request is HTTPS, verified against the public roots every browser
carries. Those roots cannot vouch for a daemon on your own machine, on your
own network, holding a certificate no public authority would sign for a
private address. For that one case the runtime also reads owner-installed
roots -- `/mnt/onboard/.adds/cobalt/trust` on the device,
`~/.config/kobo/trust` for the host runtimes -- and verifies against them
exactly as it verifies any public host, rather than offering a switch that
turns verification off.

```sh
kobo trust set sidekick --device 192.168.1.5
```

The value travels over the same attended channel as a secret, but unlike a
secret it is checked for being a PEM certificate before it goes, and listing
installed roots is harmless. Roots are loaded once at session start. The
command is rarely needed by hand: `kobo setup` carries everything in
`~/.config/kobo/trust` onto the reader it is setting up.

---

## 6. Typing, where it is unavoidable

Tapping beats typing on this panel, so a screen asks a question with `choose`
wherever it can. When words are genuinely required, the keyboard is a composite
rather than a node: rows of ordinary tappable cells and a small state machine.

```rust
use kobo_sdk::keyboard::{TextEntry, Typing};

match self.entry.handle(action) {
    Some(Typing::Changed)      => self.show(context), // repaint the field
    Some(Typing::Submitted(s)) => self.search(&s, context),
    Some(Typing::Cancelled)    => self.show(context),
    None => {}                                        // not a keyboard tap
}
```

Keys are addressed **positionally**: `kb.r1c2` is the third key of the middle
row, whatever it currently says. Shift and the symbol layer change every label
without moving a single cell, so a finger already resting on a key does not
have to be lifted and re-aimed.

---

## 7. State that survives being closed

There is no save button and there should not be. An E Ink device is closed by
shutting a cover and forgotten until the battery is flat, so any design that
depends on a clean exit loses data.

```rust
context.store().save("items", self.encode());
context.store().load("items");
context.store().forget("items");
context.store().list();
```

Every call answers exactly once at `on_store`, including its failures. Writes
are atomic, so the worst a power loss can cost is the change that was in
flight. The application names a key and never a path: where the bytes live, and
that they cannot be another application's bytes, is the runtime's problem.

---

## 8. A terminal

The shell is a runtime capability, not something an application opens. There is
no pseudo-terminal in the SDK, no fork, no file descriptor and no way to name a
program:

```rust
let (columns, rows) = kobo_sdk::terminal_grid_for(&empty_screen, &context.metrics());
context.shell().open(columns, rows);
context.shell().input(bytes);      // exactly what was typed
context.shell().resize(c, r);
context.shell().close();
```

Everything the program has to say arrives at `on_shell_event`: `Opened`,
`Output(bytes)`, `Closed { status }`, or `Refused(error)`. Feed the output into
`kobo_term::Terminal` and draw its `rows()` and `cursor()` with
`ScreenBuilder::terminal`.

Ask `terminal_grid_for` for the grid rather than computing one. It lays the
screen out with an empty terminal and measures what is left, so the program
wraps its lines exactly where the reader sees them wrap; an application that
did its own arithmetic about bars and keyboards would be wrong the first time
either changed.

`terminal_keys` sends a byte the instant a key is tapped rather than collecting
a word, because `Ctrl-C` has to arrive while the program is still running.
`Ctrl` is plain arithmetic: it clears the two high bits,
which is why `Ctrl-C` is 3 and `Ctrl-[` is escape. Return sends a carriage
return, and the key above it sends delete.

This is the one capability that is different in kind from the rest. Everything
else this platform does is undone by a reboot; a shell here is root on a
writable root filesystem. It is refused unless the application holds
`Capability::Shell`, and the runtime stops the program when the application
goes away, so a crash cannot leave a root shell running with nothing attached.

---

## 9. Leaving, and coming back

Leaving an application does not end it. It is put behind the launcher, so a
download or a build keeps running and returning is a repaint rather than a
restart.

```rust
fn on_background(&mut self, context: &mut Context) {
    // Nothing drawn now will be seen. Write anything that must not be lost.
}

fn on_foreground(&mut self, context: &mut Context) {
    // The panel still holds the last thing this application drew, so there is
    // no blank to cover, but anything that changed while away must be drawn.
    self.show(context);
}
```

Drawing while backgrounded is not an error, it is just traffic for no picture.
A long-running job should keep its state and rebuild the screen once on the way
back, instead of sending one per chunk of progress.

---

## 10. Hardware

```rust
context.device().read_battery();
context.device().read_battery_detail();
context.device().read_identity();
context.device().read_cover();
context.device().hold_wifi(Duration::from_secs(60));
context.device().set_frontlight(40);
context.device().set_bluetooth(true);
context.device().scan_bluetooth();
context.device().pair_bluetooth("AA:BB:CC:DD:EE:FF");
context.device().scan_wifi();
context.device().join_wifi("Library", "eight-or-more");
context.device().load_shelf_audio("chaptered-book.mp3z");
context.device().play_audio();
context.device().seek_audio(Duration::from_secs(30));
```

Every one is a request, answered at `on_device_result` with a `DeviceResult`
that may be `Denied`. There are three distinct refusals and they mean different
things:

- **`NotDeclared`**. The application did not ask for the capability.
- **`WithheldForBattery`**. Policy will not spend the charge right now.
- **`Unsupported`**. This build genuinely cannot do it.

A build performs only what it has a proven backend for. The device backend
uses the firmware's running `wpa_supplicant` and Bluetooth service; it does not
start a second network owner or manipulate HCI/module state. Radio answers are
typed `DeviceResult::Wifi` and `DeviceResult::Bluetooth` values, while backend
failures are `DeviceResult::Failed(DeviceError)`. On a MediaTek Clara, using
an active Bluetooth stack makes the runtime reboot cleanly when the Cobalt
session ends because handing the initialised vendor driver directly back to
Nickel is not safe. **An invented reading is worse than a refusal**, because an
application cannot tell one from the other and will act on it.

### Declaring capabilities

`context.device()` only grants what the manifest declares, and the runtime
clamps even that:

| Capability | Purpose |
| --- | --- |
| `network` | Reach the network in the foreground |
| `background-network` | Reach the network from a scheduled wake |
| `hold-wifi` | Keep Wi-Fi associated, for always-on dashboards |
| `keep-awake` | Stay out of suspend in the foreground |
| `scheduled-wake` | Be woken to refresh content |
| `battery-read` | Read battery percentage and charging state |
| `frontlight-control` | Change front light brightness |
| `audio`, `bluetooth-audio` | Play audio, including to headphones |
| `bluetooth-control` | Power, scan, pair and connect Bluetooth devices |
| `wifi-control` | Power, scan, join and disconnect Wi-Fi |
| `sleep-screen` | Draw the sleep screen |
| `notifications` | Post notifications |
| `shared-files` | Use a user-visible folder |
| `shell` | Run a terminal, hosted by the runtime |

Unknown names are rejected rather than ignored, dependencies are enforced
(`hold-wifi` requires `network`, `background-network` requires
`scheduled-wake`, `bluetooth-audio` requires `audio`), and a system
`PowerPolicy` the application cannot raise imposes a minimum wake interval, a
maximum Wi-Fi hold, and withdrawal of the expensive capabilities below fifteen
percent battery unless the device is charging.

`sleep-screen`, `notifications` and `shared-files` are reserved capability
names in the policy and manifest format, but `kobo-sdk` does not currently
expose an application call for them. Do not request them yet. Large
application-private files use `Context::shelf`; that is not the user-visible
shared-files surface.

`shell` is the one that is different in kind. Every other capability is undone
by a reboot; a shell on this device is root on a writable root filesystem, so
it is the first thing the platform hosts that a power cycle cannot repair. It
is never implied by another capability, it is granted today only to the
application named `terminal`, and the application never holds the
pseudo-terminal: it sends what was typed and receives what was printed, so the
runtime is the only thing that can start, bound, or stop a program.

### The cover sensor

There is a hall sensor behind one edge of the bezel. It is what a sleep cover
closes against, but it cannot tell a cover from any other magnet, so the SDK
reports what was measured and leaves the meaning to you.

```rust
fn on_start(&mut self, context: &mut Context) {
    context.device().read_cover();
}

fn on_cover_change(&mut self, context: &mut Context, magnet_present: bool) {
    self.present = magnet_present;
    context.set_screen(self.screen());
}
```

Two things follow from how the hardware works and both will bite an
application that ignores them:

- **Edges are not the state.** A magnet that was already there when your
  application started produced no event and never will. Ask `read_cover` once
  at the start; after that, changes arrive on their own.
- **Only the foreground application hears it.** A magnet arriving is something
  that happened in front of the reader, so a backgrounded application is not
  told and must ask again when it returns.

The runtime settles the sensor's bounce before telling anyone, so what arrives
is movement rather than noise. `examples/magnet` is the whole surface on one
screen, and doubles as the calibration screen: nothing on the case says where
the sensor is, so you walk a magnet along the edges and watch for the answer.

For a complete playback screen, use the SDK component rather than rebuilding
transport and pairing state:

```rust
use kobo_sdk::audio::{AudioMetadata, AudioPlayer};

let mut player = AudioPlayer::shelf("chaptered-book.mp3z", "Night Sky")
    .metadata(AudioMetadata::new("Night Sky").author("Ada Example"));
player.start(context);

// In on_action:
if player.press(context, action) {
    context.set_screen(player.screen());
}

// Forward on_device_result and on_task in the same way. The latter owns only
// its scan-delay and five-second position-poll task IDs, so application tasks
// remain distinguishable.
```

The component accepts an optional cached `TilePicture` cover. Its Play action
uses an already-connected Bluetooth audio-class device; without one, it opens
its embedded headphones/speaker picker, powers and scans Bluetooth if needed,
pairs and connects the selected output, and resumes the pending Play action.
Keyboard and remote connections do not count as audio outputs. Shelf paths are
resolved within the calling application's shelf, and stream sources are
unauthenticated HTTPS objects cached under a 64 MiB ceiling before playback.

`AudioPlayer::owns_back(true)` is for a player reached from a list rather than
opened as the application's front door. Without it, a player sitting at the
root of a single-application session is drawn with no back control at all, so
tapping a book is a one way door. With it the application receives
`ActionId::BACK` and answers with the list it came from.

The author is the hero's byline and is stated exactly once. It is deliberately
not repeated as a fact row two lines below.

---

## 11. Running it

```sh
# In a browser, against the same renderer the device uses.
cargo run -p kobo-cli -- dev --builtin

# Against the real runtime, on a host socket.
cargo run -p kobo-cli -- run --sim

# For the device.
CC_armv7_unknown_linux_musleabihf=arm-linux-gnueabihf-gcc \
  cargo build --release --target armv7-unknown-linux-musleabihf -p your-app

# For somebody else's device, with no terminal at their end.
cargo run -p kobo-cli -- package
```

The Rust target plus an ARM hard-float C compiler are the cross-build setup.
Binaries are statically linked and have no device-side dependencies.

The simulator performs real work rather than faking it. A fetch is a real
request, a terminal is a real shell on the developer's own machine, and the
type is the same face the panel uses, compiled in so that two developers on
different machines see the same line breaks. An application that could only
reach the network on the device could only be built on the device, which is
the one thing this is arranged to avoid.

The current target is the measured `clara-bw-391` profile: 1072 × 1448 at 300
PPI, rotation 3, with display taps converted through the Clara's raw controller
ranges before SDK hit testing. The runtime and simulator share the exact Rust
refresh planner, including dirty rectangles, DU/GL16/GC16 selection and an
eight-partial-update cleaning cadence. The browser's visible residue is an
explicit approximation (an LCD cannot reproduce electrophoretic physics) and
the **Show ideal pixels** control makes that boundary inspectable. Keeping this
authoritative logic in the native simulator avoids adding a second WASM build
and download to the development loop; a Rust-to-WASM renderer can be added
later for a fully static/offline embed without changing the simulation model.

Failure handling is still code that has to run. Select a deterministic scenario
in the inspector to exercise offline, low-battery, permission-denied,
missing-secret, network-timeout, storage-full and image-cache-pressure paths.
The foreground and background buttons deliver real SDK lifecycle messages.
`KOBO_SIM_OFFLINE=1` remains available for headless or scripted development.

The simulator's diagnostics panel is part of the normal development loop. It
reports content beyond the fold, clipping, undersized targets, text overflow,
unsupported characters, duplicate node IDs, truncated collections, invalid
picture sizes and missing cache entries. Toggle **Show diagnostic outlines**
to draw each issue over the exact rectangle returned by the shared layout
engine. The adjacent panel inspector names every refresh waveform, whether it
is partial or cleaning, its pixel rectangle, refresh count and accumulated
partials. Raw and display touch coordinates are shown after every tap.

`package` produces the single `KoboRoot.tgz` an owner copies into `.kobo/` over
USB; the reader installs it at the next boot. Everything lands in
`.adds/cobalt` on the book partition, which is mounted without `noexec`, so no
rootfs file and no boot script is involved and uninstall is deleting a folder.
`kobo inspect` reads a built package back and refuses one that could write
anywhere else.

### Connecting a device

Everything above runs on a host. Nothing in this section is needed to write an
application, and it is here because the first hour with a real reader is
otherwise spent guessing.

A Kobo powers its radio down whenever it sleeps, and takes a new address from
DHCP every time the radio comes back. So the address you had is a guess, and
the only symptom of any of it is a connection timeout. `kobo devices` sweeps the
local network, reads four files off whatever answers on port 22, and names the
readers it found:

```sh
kobo devices
kobo devices --subnet 192.168.1
```

```
192.168.1.15  N365 · firmware 4.45.23697 · Cobalt 0.1.0
```

A device also stops answering a few minutes after anybody stops touching it.
Two settings hold it open while you work, and both clear on a reboot:

```sh
kobo session --device <address> --wifi-always-on on
kobo session --device <address> --keep-awake on
```

With SSH already working on the device, an install needs no cable and no
reboot:

```sh
cargo run -p kobo-cli -- deploy --device <address>
```

`deploy` builds the same archive `package` builds and sends it over the
stdin-only shell channel as base64; the device checks the SHA-256 of what
arrived against the SHA-256 of what was sent, refuses any path outside
`.adds/cobalt`, and refuses outright while a Cobalt session is running, because
the files it would replace are the ones being executed. It needs no reboot
because the vendor installer is not involved: the binaries simply land on the
book partition and run from there.

`package` remains the path for somebody else's device, and the one to use if
this will not connect. **Cobalt does not install an SSH server and does not
need one to run.** SSH is only how a developer's machine reaches a device.

### Driving it, and photographing the result

Everything above tells you how to *run* an application. This is how to check
what it actually looks like, without a person watching it.

The gap this closes is specific. A layout assertion proves a button was placed;
it does not prove the screen reads as a product, and it does not prove the
button is reachable. The only thing that answers either question is to drive
the application the way a finger does and then look at the result, which is
also exactly the loop something automating on your behalf needs, and it was the
one loop this SDK had no way to close.

```sh
# One terminal: the app, in the simulator.
cd examples/gutenbird && cargo run -p kobo-cli -- dev 127.0.0.1:8787

# Another: drive it, and bring pictures back.
cargo run -p kobo-cli -- drive --script tour.kobo --shots target/shots
cargo run -p kobo-cli -- drive --step "tap Search" --step "shot search"
```

A script is one step per line; `#` is a comment.

| step | what it does |
| --- | --- |
| `tap LABEL` | finds the node whose text carries `LABEL` and taps its centre |
| `tap-at X,Y` | taps a point, for a control with no words |
| `type TEXT` | taps the on-screen key for each character in turn |
| `expect TEXT` | fails unless something on the screen says it |
| `expect-missing TEXT` | fails if something does |
| `wait-for TEXT` | the same as `expect`, but allows for work in flight |
| `clean` | fails if the renderer raised any error about this screen |
| `shot NAME` | writes `NAME.png` into the shots folder |
| `dump` | prints every node and its text, for writing the next step |
| `scenario NAME` | switches to a deterministic failure scenario |
| `lifecycle background` / `foreground` | delivers a real lifecycle message |
| `wait MS` | waits |

A failing step reports the line, the step and the reason, and takes a
screenshot first, because the question that immediately follows "tap Search
failed" is "what was on the screen".

**`--record` films the run instead of photographing it.**

```sh
cargo run -p kobo-cli -- drive --script drive.kobo --record target/demo --fps 4
```

The panel is sampled on its own clock while the script drives, so the
recording catches the screens a run passes through as well as the ones it stops
on: the list that was empty for half a second before the fetch came back, the
pane drawn at the wrong size until the second layout pass. Frames identical to
the one before them are dropped, so a script that waits two seconds between
taps is one frame there and not eight. The directory ends up holding numbered
greyscale PNGs, a `timings.txt`, a `recording.mp4` and a `recording.gif` --
the same shape `kobo record --device` produces from a reader, so a demo made
here and a demo made on hardware are the same kind of file.

A recording is taken from the residue-free frame **unless you pass
`--ghosting`**, which is the opposite default to `shot` and deliberate: a still
with e-ink residue on it is two screens a person can read past, and a hundred
of them played in sequence is every screen of the run at once. `--ideal` goes
on governing the `shot` steps.

ffmpeg assembles both files and is checked for before the script runs, because
finding out it is missing after a minute of driving costs the minute.

`scripts/record-apps-sim.sh` does this for every application in the catalog,
one simulator at a time, with no reader anywhere near it.

**Pass `--ideal` when you are reading the screenshots rather than the
refreshes.** A screenshot is taken from the simulated panel, and the simulated
panel keeps the e-ink residue of what it drew last, exactly as the device
does. That is what you want when the question is whether a screen refreshes
cleanly, and exactly what you do not want when the question is whether it
*reads* well: two screens overlaid are hard for a person to judge and worse
for a model. `--ideal` takes the frame without the residue. `kobo shot`
accepts it too.

**`tap` resolves a label to a coordinate and then taps the coordinate.** It
would have been simpler to dispatch the action directly and it would have been
worthless: that passes happily on a screen whose only button has been laid out
four millimetres below the bottom edge of the panel, which is precisely the
fault worth catching. The tap goes through the panel's own touch transform and
the renderer's own hit-testing, so if the control is not reachable, the tap
misses and the script fails.

`type` is the same argument. This device has no hardware keyboard, so text
injected into the application would be exercising a path no reader can take.
Each character is a key that has to be on the screen; if it is not, that is a
finding.

For the real panel:

```sh
# A PNG of whatever is on the e-ink display right now.
cargo run -p kobo-cli -- shot --device <address> --out screen.png

# A real tap on the real glass.
cargo run -p kobo-cli --features device-write -- tap --device <address> 536,900
```

`shot --device` is read-only: it opens the framebuffer for reading, copies it,
and closes it. Nothing is grabbed, nothing is refreshed and no pixel is
written, so it is safe to point at a device with the stock reader in the
foreground, which matters, because the screen worth photographing is usually
the one that has just gone wrong and must not be disturbed to be seen. The
panel comes back as base64 grey with its measured width, height and length, so
a transfer cut short is refused rather than saved as half a picture.

`tap --device` writes real evdev records to the real touch node, so the
digitiser's coordinate space, the profile's `display_to_touch` transform, the
multitouch decoder and the hit-testing all run exactly as they do under a
finger. It is behind `device-write` and behind an unlock phrase, because a
program that can tap anything can tap the stock reader's factory reset. It taps
once, at one point, which must be on the screen, and it always lifts: a tap
that failed halfway would leave the digitiser reporting a finger that is not
there.

The division is deliberate. **`drive` is the simulator; `shot`, `tap` and
`record` are the device.** Resolving a label to a coordinate needs the layout,
and the layout lives in the process doing the rendering. On the host that is
the simulator, which runs the identical renderer, layout engine, hit-testing
and refresh planner. On the device it is inside the running application, and
opening a control channel into it in order to test it would be testing
something other than the shipped path.

That is why filming the simulator is `drive --record` and not `record
--address`. `record` is a framebuffer reader: it uploads the doctor, has the
device copy `/dev/fb0` into a file at a fixed rate, and pulls that file home
over SSH compressed. None of those three things has a counterpart on this side
of the cable, and there is no framebuffer here to read. What there is, is a
driver already holding a connection to the process doing the rendering.

---

## 12. The rules the SDK will not let you break

On a reader these are enforced by a root-owned, per-process chroot, an
unprivileged UID, `no_new_privs`, network syscall isolation, validated binary
identity, denied child-process creation and runtime brokers. The host simulator
is a development tool running as the developer's own user; it exercises the
protocol and policies but is not an operating-system security boundary.

- **You cannot draw.** No pixels, no colour, no fonts, no coordinates.
- **You cannot block the panel.** Long work is a task or it does not happen.
- **You cannot hold a credential.** You may name one.
- **You cannot remove Back.** The runtime owns it, draws it, and leaves to the
  launcher if you do not answer it. You may ask to answer it first.
- **You cannot open a socket, a file outside your directory, or a device node.**
- **You cannot start a program.** A terminal is a capability the runtime hosts;
  an application says what was typed, never what to execute.
- **You cannot ship an illegible icon.** The set is closed and drawn by the
  runtime.

Each of these is a thing that, left to an application, eventually produces a
device its owner cannot get out of. The rest of the design follows from a
single rule: **nothing that cannot be undone by a reboot.**

---

## 13. Where things live

| Crate | What it is |
|---|---|
| `kobo-sdk` | What an application imports. `ScreenBuilder`, `KoboApp`, `Context`. |
| `kobo-ui` | Layout, rendering, pagination, vector icons. Shared by app and runtime. |
| `kobo-protocol` | The bounded wire format between the two. |
| `kobo-policy` | Capabilities, the task runner, device services, keyed storage. |
| `kobo-net` | HTTPS. Carries TLS so nothing else has to. |
| `kobo-json` | A small JSON reader and object builder. |
| `kobo-html` | Defensive HTML-to-text conversion and optional formula rasterisation. |
| `kobo-xml` | A bounded XML pull scanner for feeds and Atom documents. |
| `kobo-opds` | OPDS 1.2 and 2.0 catalogue parsing. |
| `kobo-image` | Bounded JPEG/PNG decoding, fitting and E Ink dithering. |
| `kobo-doc` | EPUB, HTML, Markdown and plain-text bytes into a structured document. |
| `kobo-read` | Pagination, reading positions, contents, search and annotations. |
| `kobo-bookview` | End-to-end SDK reading surface, including deferred pictures. |
| `kobo-text` | Typeface loading and measurement. |
| `kobo-shell` | One terminal per application, hosted by the runtime. |
| `kobo-term` | The vt100 screen a program's output is parsed into. |
| `kobo-hal` | Display, touch, battery, reader handoff. |
| `kobod` | The runtime. Owns the panel, the session and everything refusable. |
| `kobo-sim` | The browser simulator, using the same renderer. |
| `kobo-cli` | Scaffolding, simulation, building, diagnostics. |

Plus four device-side tools that are never linked into an application:
`kobo-doctor` (read-only identity probe), `kobo-smoke` (owner-attended display
writes), `kobo-handoff` (stopping and restarting the stock reader) and
`kobo-guard` (screen capture and restore around a session). `kobo-abi` and
`kobo-profile` sit under `kobo-hal`: the only `unsafe` in the workspace, and the
exact hardware identity that gates it.

External dependencies live behind narrow interfaces in `kobo-net`,
`kobo-text`, `kobo-term`, `kobo-image`, `kobo-doc` and `kobo-abi`; applications
do not depend on the particular HTTP, font, terminal, image or kernel binding
implementation.

Worked examples, smallest first: `examples/tictactoe`, `examples/todo` (state
that survives a restart), `examples/gallery` (every primitive on one screen),
`examples/terminal`, `examples/brief` (work that continues in the background),
`examples/launcher`, `examples/chat`, `examples/gutenbird`.
