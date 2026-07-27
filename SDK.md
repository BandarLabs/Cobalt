# The Kobo application SDK

Write an application for a Kobo e-reader in one file, with no dependencies
outside this workspace, and run it on the panel.

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

## 3. Building a screen

`ScreenBuilder` is a chain. Every method that adds content returns `Self`.

### Structure

| Method | What it is |
|---|---|
| `top_bar(title)` | The fixed bar at the top. Back is added by the runtime. |
| `top_bar_action(name, label)` | One trailing control in the top bar. |
| `nav_bar(selected, [(name, label), …])` | The pinned bottom bar. At least two destinations. |
| `page_turns(previous, next)` | Tap the left of the page to go back, the rest to go on. |

### Content

| Method | What it is |
|---|---|
| `heading(text)` | One line of display type. |
| `text(text)` | A paragraph. Wraps at a measure derived from physical width. |
| `button(name, label)` | A full-width action. |
| `rows([(name, title, summary, glyph), …])` | A list. Title, one line of detail, an icon. |
| `checklist([(name, title, summary, done), …])` | The same list, where a finished row is struck through. |
| `tiles([(name, label, glyph), …])` | A grid of square destinations. |
| `grid(columns, square, cells)` | A board. What tic-tac-toe is drawn with. |
| `choose(prompt, [(name, label), …])` | A question with tappable answers. |
| `chosen(index)` | Marks which answer of the preceding `choose` is already given. |
| `or_type(name, placeholder)` | A freeform row on the end of a `choose`. |
| `quote(depth, text)` | A paragraph set in by reply depth, with a gutter rule. |
| `picture(picture, max_height_mm)` | One picture, as large as the width and that height allow. |
| `picture_tiles(shape, […])` | A grid of tiles that each carry a picture, falling back to a glyph. |
| `banner(level, text)` | `Info` or `Attention`. Attention is drawn inverted. || `progress(percent)` | A determinate bar. |
| `activity(label, progress)` | An indeterminate wait. |
| `cancellable(name, label)` | Adds a cancel control to the preceding activity. |
| `skeleton(lines)` | Placeholder lines, occupying where content will land. |
| `divider()` / `spacer(space)` | Rules and space, from the spacing scale. |
| `paged_list(page, items)` | A pre-paged list of plain strings. |
| `keyboard(&keyboard, submit)` | The on-screen keys. Positional, so a layer change moves nothing. |
| `text_entry(&entry, prompt, submit)` | A prompt, what has been typed, and the keys. |
| `terminal(rows, cursor)` | A character grid with a block caret. |
| `terminal_keys(&keys)` | Keys that send a byte the moment they are tapped. |

There is no free-form drawing, no colour, no font choice and no pixel
positioning. Every size comes from the panel's *physical* dimensions, so a
control that is comfortable under a thumb on a six inch panel is comfortable on
a ten inch one, and a line of text holds roughly the same number of words on
both.

### Icons

`Glyph` is a closed set: `App`, `Book`, `Note`, `Clock`, `Settings`, `Folder`,
`Chart`, `Search`, `Wifi`, `Battery`, `Reader`, `Power`, `Grid`, `Circle`,
`Check`, `Terminal`, `Chat`, `News`.

They are geometry, not bitmaps — authored in a 1000 unit box and rasterised
with coverage antialiasing at whatever size the layout asks for, so they are
crisp at every density. Applications cannot supply their own paths: arbitrary
path data is untrusted input to a rasteriser, and an application must not be
able to draw something indistinguishable from a system control.

### State is carried, never drawn into a label

A finished row, a chosen answer and a reply's depth are all *state*: the
application says what is true and the renderer decides what it looks like.
There is no way to ask for a line through text, a tick beside a label or an
indent, and that is deliberate — an application that marks its own choice with
a character picks one the installed face may not have, and gets an empty box on
the panel. In debug builds `set_screen` refuses a screen carrying a character
the face cannot draw, so an application's own tests fail rather than the panel.

### Threaded replies

```rust
for (depth, paragraph) in &page {
    screen = screen.quote(*depth, paragraph);
}
```

`quote` sets a paragraph in by one step per level, up to `MAX_QUOTE_DEPTH`,
with a rule down the gutter. Paginate the same shape with
`context.paginate_quoted(&paragraphs, nav_bar)` — an indented paragraph is
narrower, wraps to more lines and eats more of the page, so a thread paginated
flat and drawn indented loses the bottom of nearly every page.

### One-line labels

`context.one_line_row(text, nav_bar)` measures against the real installed face
and ellipsises, so a list of headlines has rows of uniform height instead of
some one line tall and some three.

### Pictures

`kobo-image` decodes JPEG and PNG on the host and on the device, halftones to
the sixteen greys the panel resolves with `dither(PANEL_GREYS)`, and fits a
picture to the cell it will occupy. `fit` shrinks only; `fit_enlarging` will
blow a small picture up to `MAX_ENLARGEMENT` times, which is what a book cover
published at 190 by 300 needs to fill a tile on a 300 pixel-per-inch panel.
The application decodes and hands the runtime the pixels at the size they will
be drawn, so the picture cache holds what is on the screen rather than what was
downloaded.

---

## 4. Pagination, and why there is no scrolling

There is no scroll view and there should not be. A panel that takes the better
part of a second to repaint cannot follow a finger, and a partial refresh
chasing a moving list is precisely the operation that leaves ghosting behind.

There is also a trap: **the layout engine silently drops whatever does not
fit.** It stops placing nodes once the cursor passes the bottom of the content
area, and nothing tells you. So anything that must stay reachable belongs in
the nav bar, which is reserved before content is placed — never at the end of
the flow, where it is the first thing to be dropped.

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

`spawn` returns immediately with an `Option<TaskId>` — `None` if the runtime
would not even queue it — and the result arrives at `on_task`:

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

- **`Fetch { url, offset, max_bytes }`** — HTTPS only. `offset` reads a long
  document in pieces; a range is sent for every piece including the first.
- **`Post { url, body, content_type, secret, max_bytes }`** — `secret` is the
  *name* of a credential the runtime holds. Never its value.
- **`ReadFile { path }`** — confined to the application's own directory.
- **`Sleep { seconds }`** — waits without holding a wake lock.

Show that something is happening. `activity(label, None)` plus `skeleton(n)`
puts a placeholder where the content will land, which reads far better on a
slow panel than an empty screen that suddenly fills.

### Credentials

```rust
Task::Post { secret: Some("openai".into()), .. }
```

```rust
Task::Post { credential: Some(Credential::in_header("anthropic", "x-api-key")), .. }
```

The application names a secret; the runtime reads
`/mnt/onboard/.adds/cobalt/secrets/<name>` and attaches it, either as a bearer
token or under the header the application named — which is what lets a request
go straight to Anthropic or Gemini rather than through a proxy.
The value is never in the application's memory, its logs, or its crash dump,
and it cannot be sent anywhere the application did not name — the request is
not replayed across a redirect.

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

Keys are addressed **positionally** — `kb.r1c2` is the third key of the middle
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
`Ctrl` is arithmetic rather than a lookup table — it clears the two high bits,
which is why `Ctrl-C` is 3 and `Ctrl-[` is escape — return sends a carriage
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
    // no blank to cover — but anything that changed while away must be drawn.
    self.show(context);
}
```

Drawing while backgrounded is not an error, it is just traffic for no picture.
A long-running job should keep its state and rebuild the screen once on the way
back, rather than sending one per chunk of progress.

---

## 10. Hardware

```rust
context.device().read_battery();
context.device().hold_wifi(Duration::from_secs(60));
context.device().set_frontlight(40);
```

Every one is a request, answered at `on_device_result` with a `DeviceResult`
that may be `Denied`. There are three distinct refusals and they mean different
things:

- **`NotDeclared`** — the application did not ask for the capability.
- **`WithheldForBattery`** — policy will not spend the charge right now.
- **`Unsupported`** — this build genuinely cannot do it.

A build performs only what it has a proven backend for. Today that is the
read-only battery gauge; everything else is honestly refused rather than
answered with a plausible invention. **An invented reading is worse than a
refusal**, because an application cannot tell one from the other and will act
on it.

---

## 11. Running it

```sh
# In a browser, against the same renderer the device uses.
cargo run -p kobo-cli -- dev --builtin

# Against the real runtime, on a host socket.
cargo run -p kobo-cli -- run --sim

# For the device.
cargo build --release --target armv7-unknown-linux-musleabihf -p your-app

# For somebody else's device, with no terminal at their end.
cargo run -p kobo-cli -- package
```

`rustup target add armv7-unknown-linux-musleabihf` is the entire cross-build
setup. Binaries are statically linked and have no device-side dependencies.

The simulator performs real work rather than faking it. A fetch is a real
request, a terminal is a real shell on the developer's own machine, and the
type is the same face the panel uses, compiled in so that two developers on
different machines see the same line breaks. An application that could only
reach the network on the device could only be built on the device, which is
the one thing this is arranged to avoid.

Failure handling is still code that has to run, so set `KOBO_SIM_OFFLINE=1` to
make every network task fail. Deliberately, rather than always.

`package` produces the single `KoboRoot.tgz` an owner copies into `.kobo/` over
USB; the reader installs it at the next boot. Everything lands in
`.adds/cobalt` on the book partition, which is mounted without `noexec`, so no
rootfs file and no boot script is involved and uninstall is deleting a folder.
`kobo inspect` reads a built package back and refuses one that could write
anywhere else.

---

## 12. The rules the SDK will not let you break

- **You cannot draw.** No pixels, no colour, no fonts, no coordinates.
- **You cannot block the panel.** Long work is a task or it does not happen.
- **You cannot hold a credential.** You may name one.
- **You cannot remove Back.** The runtime owns the navigation stack.
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

Outside dependencies live in exactly three of those crates, each behind one
interface: `kobo-net`, `kobo-text` and `kobo-term`. Nothing an application
imports has any.

Worked examples, smallest first: `examples/tictactoe`, `examples/todo` (state
that survives a restart), `examples/gallery` (every primitive on one screen),
`examples/terminal`, `examples/brief` (work that continues in the background),
`examples/launcher`, `examples/chat`, `examples/gutenshelf`.
