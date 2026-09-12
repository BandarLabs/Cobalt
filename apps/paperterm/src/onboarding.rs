//! First-run guidance and an entirely local terminal preview.
use kobo_sdk::keyboard::Keyboard;
use kobo_sdk::{Screen, ScreenBuilder, Space};

pub(super) fn welcome(error: Option<&str>) -> Screen {
    ScreenBuilder::new("paperterm-welcome")
        .top_bar("Paperterm")
        .heading("Terminal sessions")
        .text("Read a terminal session on your Kobo. Type from either device when keyboard access is enabled.")
        .button("setup", "Connect a computer")
        .button("preview", "Try a preview")
        .secondary(error.unwrap_or("The computer runs the session. Keep it awake and on the same network."))
        .build()
}
pub(super) fn setup() -> Screen {
    ScreenBuilder::new("paperterm-setup")
        .top_bar("Paperterm")
        .heading("1. Prepare the computer")
        .text("Open its terminal and run:")
        .text("kobo stream init")
        .text("Keep the address and pairing code it prints. Both devices need to be on the same network.")
        .button("trust", "Next: trust this computer")
        .button("welcome", "Back")
        .build()
}
pub(super) fn trust() -> Screen {
    ScreenBuilder::new("paperterm-trust")
        .top_bar("Paperterm")
        .heading("2. Trust this computer")
        .text("Run kobo devices on your computer to find the reader's address. Use that address for READER_IP:")
        .text("kobo trust set stream --device READER_IP")
        .text("This lets your Kobo recognize the computer's secure connection.")
        .button("start", "Next: start a session")
        .button("setup", "Back")
        .build()
}
pub(super) fn start() -> Screen {
    ScreenBuilder::new("paperterm-start")
        .top_bar("Paperterm")
        .heading("3. Start a session")
        .text("For keyboard access on both devices, run:")
        .text("kobo stream --interactive -- /bin/sh")
        .text("Leave this terminal open and keep the computer awake. The session runs on the computer.")
        .button("enter-address", "Enter computer address")
        .button("trust", "Back")
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
