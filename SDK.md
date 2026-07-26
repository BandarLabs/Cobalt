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
| `tiles([(name, label, glyph), …])` | A grid of square destinations. |
| `grid(columns, square, cells)` | A board. What tic-tac-toe is drawn with. |
| `choose(prompt, [(name, label), …])` | A question with tappable answers. |
| `or_type(name, placeholder)` | A freeform row on the end of a `choose`. |
| `banner(level, text)` | `Info` or `Attention`. Attention is drawn inverted. || `progress(percent)` | A determinate bar. |
| `activity(label, progress)` | An indeterminate wait. |
| `cancellable(name, label)` | Adds a cancel control to the preceding activity. |
| `skeleton(lines)` | Placeholder lines, occupying where content will land. |
| `divider()` / `spacer(space)` | Rules and space, from the spacing scale. |
| `paged_list(page, items)` | A pre-paged list of plain strings. |

There is no free-form drawing, no colour, no font choice and no pixel
positioning. Every size comes from the panel's *physical* dimensions, so a
control that is comfortable under a thumb on a six inch panel is comfortable on
a ten inch one, and a line of text holds roughly the same number of words on
both.

### Icons

`Glyph` is a closed set: `App`, `Book`, `Note`, `Clock`, `Settings`, `Folder`,
`Chart`, `Search`, `Wifi`, `Battery`, `Reader`, `Power`, `Grid`.

They are geometry, not bitmaps — authored in a 1000 unit box and rasterised
with coverage antialiasing at whatever size the layout asks for, so they are
crisp at every density. Applications cannot supply their own paths: arbitrary
path data is untrusted input to a rasteriser, and an application must not be
able to draw something indistinguishable from a system control.

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

`spawn` returns a `TaskId` immediately and the result arrives at `on_task`:

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

The application names a secret; the runtime reads
`/mnt/onboard/.adds/cobalt/secrets/openai` and attaches it as a bearer token.
The value is never in the application's memory, its logs, or its crash dump,
and it cannot be sent anywhere the application did not name — the request is
not replayed across a redirect.

---

## 6. Hardware

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

## 7. Running it

```sh
# In a browser, against the same renderer the device uses.
cargo run -p kobo-cli -- dev --builtin

# Against the real runtime, on a host socket.
cargo run -p kobo-cli -- run --sim

# For the device.
cargo build --release --target armv7-unknown-linux-musleabihf -p your-app
```

`rustup target add armv7-unknown-linux-musleabihf` is the entire cross-build
setup. Binaries are statically linked and have no device-side dependencies.

The simulator refuses network tasks rather than faking them, on purpose: an
application that has only ever seen invented responses is an application whose
error handling has never run.

---

## 8. The rules the SDK will not let you break

- **You cannot draw.** No pixels, no colour, no fonts, no coordinates.
- **You cannot block the panel.** Long work is a task or it does not happen.
- **You cannot hold a credential.** You may name one.
- **You cannot remove Back.** The runtime owns the navigation stack.
- **You cannot open a socket, a file outside your directory, or a device node.**
- **You cannot ship an illegible icon.** The set is closed and drawn by the
  runtime.

Each of these is a thing that, left to an application, eventually produces a
device its owner cannot get out of. The rest of the design follows from a
single rule: **nothing that cannot be undone by a reboot.**

---

## 9. Where things live

| Crate | What it is |
|---|---|
| `kobo-sdk` | What an application imports. `ScreenBuilder`, `KoboApp`, `Context`. |
| `kobo-ui` | Layout, rendering, pagination, vector icons. Shared by app and runtime. |
| `kobo-protocol` | The bounded wire format between the two. |
| `kobo-policy` | Capabilities, the task runner, device services. |
| `kobo-net` | HTTPS. The only crate with dependencies outside this workspace. |
| `kobo-json` | A small JSON reader and object builder. |
| `kobo-text` | Typeface loading and measurement. |
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

Worked examples, smallest first: `examples/hello`, `examples/counter`,
`examples/tictactoe`, `examples/gallery` (every primitive on one screen),
`examples/launcher`, `examples/chat`, `examples/gutenshelf`.
