//! First-run guidance and an entirely local terminal preview.
use kobo_sdk::keyboard::Keyboard;
use kobo_sdk::{Screen, ScreenBuilder, Space};

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
pub(super) fn setup() -> Screen {
    ScreenBuilder::new("paperterm-setup")
        .top_bar("Connect a computer")
        .heading("1. Pair the computer")
        .text("In a terminal on the computer, run:")
        .command("kobo stream init")
        .secondary("It finds this Kobo on your network, trusts the computer, and prints an address and a pairing code to enter here.")
        .buttons([("welcome", "Back"), ("trust", "Next")])
        .build()
}
/// The step `kobo stream init` already took when it found the reader. Kept as
/// its own page for a reader it could not find, which is the one case where
/// the owner has to do it by hand.
pub(super) fn trust() -> Screen {
    ScreenBuilder::new("paperterm-trust")
        .top_bar("Connect a computer")
        .heading("2. Trust the computer")
        .text("Done already if the last step found this Kobo. If it did not:")
        .command("kobo trust set stream --reader NAME")
        .secondary("NAME is what kobo pair saved this Kobo as.")
        .buttons([("setup", "Back"), ("start", "Next")])
        .build()
}
pub(super) fn start() -> Screen {
    ScreenBuilder::new("paperterm-start")
        .top_bar("Connect a computer")
        .heading("3. Start a session")
        .text("Run this and leave the terminal open:")
        .command("kobo stream --interactive -- /bin/sh")
        .secondary("The session runs on the computer, so keep it awake.")
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

pub(super) fn entry(code: bool, keyboard: &Keyboard, error: Option<&str>) -> Screen {
    let (id, title, prompt, field, placeholder, submit) = if code {
        (
            "paperterm-code",
            "Now the pairing code",
            "Enter the six characters printed by kobo stream init.",
            "code",
            "abc123",
            "Connect",
        )
    } else {
        (
            "paperterm-pairing",
            "Pair with your computer",
            "Enter the address printed by kobo stream init.",
            "address",
            "192.168.1.20:9332",
            "Next",
        )
    };
    let mut screen = ScreenBuilder::new(id)
        .top_bar("Paperterm")
        .top_bar_action("setup", "Help");
    if code {
        screen = screen.top_bar_action("edit-address", "Address");
    }
    screen
        .heading(title)
        .text(error.unwrap_or(prompt))
        .field(field, keyboard.text(), placeholder)
        .spacer(Space::Small)
        .keyboard(keyboard, submit)
        .build()
}
