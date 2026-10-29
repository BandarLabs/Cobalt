//! First-run guidance and an entirely local terminal preview.
use kobo_sdk::keyboard::Keyboard;
use kobo_sdk::{FieldKind, Screen, ScreenBuilder, Space};

pub(super) fn welcome(error: Option<&str>) -> Screen {
    ScreenBuilder::new("paperterm-welcome")
        .top_bar("Paperterm")
        .heading("Terminal sessions")
        .text("Read a terminal session on your Kobo. Type from either device when keyboard access is enabled.")
        .primary_button("setup", "Connect a computer")
        .button("preview", "Try a preview")
        .secondary(error.unwrap_or("The computer runs the session. Keep it awake and on the same network."))
        .build()
}
// These three pages are the same recipe as `kobo stream` with no arguments,
// the README and the Store steps, in the same order. They used to be three
// recipes that disagreed: one trusted the computer by hand, one started a
// shell with flags, and one ran a connection check first.
pub(super) fn setup() -> Screen {
    ScreenBuilder::new("paperterm-setup")
        .top_bar("Connect a computer")
        .heading("1. Pair once")
        .text("In a terminal on the computer, run:")
        .command("kobo stream init")
        .secondary(
            "It finds this Kobo and pairs Paperterm on it, so there is nothing to type here.",
        )
        .buttons([("welcome", "Back"), ("trust", "Next")])
        .build()
}
/// The one step the computer cannot take for the owner: the runtime reads
/// the computer's certificate when Cobalt opens, so a first pairing is only
/// trusted after Cobalt has been opened again.
pub(super) fn trust() -> Screen {
    ScreenBuilder::new("paperterm-trust")
        .top_bar("Connect a computer")
        .heading("2. Reopen Cobalt once")
        .text("After the first pairing, close Cobalt and open it again so this Kobo trusts the computer.")
        .buttons([("setup", "Back"), ("start", "Next")])
        .build()
}
pub(super) fn start() -> Screen {
    ScreenBuilder::new("paperterm-start")
        .top_bar("Connect a computer")
        .heading("3. Share a terminal")
        .text("Run this and leave the terminal open:")
        .command("kobo stream terminal")
        .secondary("Keep the computer awake. If kobo stream init could not reach this Kobo, enter the address and code it printed.")
        .buttons([("trust", "Back"), ("enter-address", "Enter address")])
        .build()
}
pub(super) fn preview() -> Screen {
    ScreenBuilder::new("paperterm-preview")
        .top_bar("Paperterm")
        .top_bar_action("welcome", "Close preview")
        .secondary("Preview · Read only")
        .terminal(
            [
                "$ ls",
                "notes.txt  reading-list.txt",
                "",
                "$ cat notes.txt",
                "Read chapter 4.",
                "",
                "$ _",
            ]
            .map(str::to_owned),
            None,
        )
        .fill()
        .text("This is sample output. Nothing is running on your computer.")
        .button("setup", "Connect a computer")
        .build()
}

/// The address or pairing code, typed on keys Paperterm draws itself.
///
/// The keys stay the application's: the same keyboard types into the
/// terminal, where every key goes to the computer as it is tapped. The field
/// carries its own name, its guidance and, in the guidance's place, what was
/// wrong with the last try, so a refused entry does not move the field or the
/// keys under the owner's thumbs.
pub(super) fn entry(code: bool, keyboard: &Keyboard, error: Option<&str>) -> Screen {
    let (id, title, label, hint, field, placeholder, submit, kind) = if code {
        (
            "paperterm-code",
            "Now the pairing code",
            "Pairing code",
            "Enter the six characters kobo stream init printed.",
            "code",
            "abc123",
            "Connect",
            FieldKind::Text,
        )
    } else {
        (
            "paperterm-pairing",
            "Pair with your computer",
            "Address",
            "Enter the address kobo stream init printed.",
            "address",
            "192.168.1.20:9332",
            "Next",
            FieldKind::Address,
        )
    };
    let mut screen = ScreenBuilder::new(id)
        .top_bar("Paperterm")
        .top_bar_action("setup", "Help");
    if code {
        screen = screen.top_bar_action("edit-address", "Address");
    }
    screen = screen
        .heading(title)
        .field(field, keyboard.text(), placeholder)
        .field_label(label)
        .field_hint(hint)
        .field_kind(kind);
    if let Some(error) = error {
        screen = screen.field_error(error);
    }
    screen
        .spacer(Space::Small)
        .keyboard(keyboard, submit)
        .build()
}
