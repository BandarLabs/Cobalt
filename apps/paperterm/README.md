# Paperterm

Show a terminal session from your computer on your Kobo, and type into it from
either side.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/terminal.png" alt="A shared terminal with the keyboard open"><br>A shared terminal with the keyboard open</td>
<td width="50%" valign="top"><img width="300" src="screenshots/welcome.png" alt="First launch"><br>First launch</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/connection-check.png" alt="The connection check, typed from both sides"><br>The connection check, typed from both sides</td>
<td width="50%" valign="top"><img width="300" src="screenshots/input-paused.png" alt="Typing paused after a timeout"><br>Typing paused after a timeout</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/reconnecting.png" alt="Reconnecting, with the output kept"><br>Reconnecting, with the output kept</td>
<td width="50%" valign="top"><img width="300" src="screenshots/preview.png" alt="The offline preview"><br>The offline preview</td>
</tr>
</table>

The shell, its programs and your credentials stay on the computer. Paperterm
only displays the session and sends keys. It cannot run commands on the
reader and does not save terminal output.

## Features

- Three modes, chosen on the computer:
  - **Read only**: the reader watches.
  - **Controls**: arrows, Enter, Esc, y, n and Ctrl-C.
  - **Keyboard**: a full on-screen keyboard, opened with **Keyboard** and
    hidden with **Close keys**.
- The terminal grid is sized to the screen and your text size. A Clara BW at
  the default size shows 75 columns by 47 rows, or 75 by 25 with the keyboard
  open. Larger text means fewer columns, not squashed letters.
- Only changed rows are redrawn.
- If the computer stops answering, the last output stays on screen under a
  **Reconnecting** notice.
- If a key might not have arrived, typing pauses until you check the terminal
  and choose **Resume typing**.
- **Try a preview** shows sample output offline.

## Setup

**Connect a computer** in the app walks through these steps one at a time.

1. On the computer, create its identity and install its certificate on the
   reader. `kobo devices` finds the reader's address.

   ```sh
   kobo stream init
   kobo trust set stream --device READER_IP
   ```

2. Check the connection. Type on either screen and the text appears on both.
   Type `exit` to finish.

   ```sh
   kobo stream demo
   ```

3. On the reader, enter the computer's address and six-character code. The
   default port is 9332. `kobo stream pairing` shows them again.

## Sharing a session

```sh
kobo stream terminal                   # your login shell
kobo stream monitor                    # top
kobo stream --interactive -- /bin/sh   # any command, with the full keyboard
kobo stream --controls -- COMMAND      # navigation keys only
kobo stream -- COMMAND                 # read only
```

All of them accept `--port PORT`. Keep the computer awake while sharing.

To stop sharing, press **Ctrl+]** in the computer's terminal. This ends the
shared command and restores your terminal settings. Ctrl-C still goes to the
shared program.

The computer reports **waiting for a reader**, **Reader connected**,
**waiting for the reader to reconnect** after 45 seconds without contact, and
**command stopped**, when the final screen stays up for one minute.

## Permissions

- `network`: connects to the computer sharing the session.

## Development

```sh
cargo test -p kobo-paperterm
python3 scripts/check-apps-sim.py paperterm
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`. The screenshots come from a live check against a local session with private test certificates. Its `--pair-on-reader`, `--load-failure`, `--save-failure` and `--temporary-pairing` options cover pairing and recovery.
