# Paperterm

Paperterm is the reader half of `kobo stream`: a terminal session rendered on
e-ink while its pty, shell, command, and credentials remain on the computer.
The app has only the `network` capability; it cannot run a shell and does not
store terminal content.

<img width="300" src="screenshots/pairing.png" alt="Paperterm pairing address field and keyboard in the Clara BW simulator">

Start the host once with `kobo stream init`, install its root with
`kobo trust set stream --device READER_IP`, then run:

```sh
kobo stream --controls -- claude
```

Paperterm requires Cobalt 0.3.5's protocol-12 orientation API. It requests
landscape once when the session starts, sends its measured landscape grid in `/hello`,
and holds the last received rows behind an `off the air` banner when the host
cannot be reached. The banner paints once on the offline transition; unchanged
retries do not repaint, and the first successful response clears it once.
Read-only sessions show no terminal input. Controls mode
offers only arrows, Enter, Esc, y, n, and Ctrl-C; full mode also exposes the
platform terminal keyboard. After the host reports its input mode, Paperterm
repeats `/hello` once with the grid measured for those exact controls rather
than reserving a hidden keyboard. The host accepts at most 64 input bytes per
request and checks that control-mode input is in this same closed list.

The mirror uses the platform terminal node and its measured grid. Received
deltas update only changed rows; an empty poll paints nothing. Text styling is
discarded except for the cursor. Unsupported glyphs become neutral
width-preserving marks, while VT box drawing, alternate-screen transitions,
and cursor-only changes retain their terminal structure. The responsive
terminal layout clips excess rows before layout, so controls and every enabled
keyboard key remain visible.
