# Terminal

A shell on your Kobo, with a touch keyboard.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/shell.png" alt="A shell listing the reader's files"><br>A shell listing the reader's files</td>
</tr>
</table>

## Features

- Runs `/bin/sh` on the reader.
- The keyboard sends each key immediately, so Ctrl-C reaches a running
  program. Esc, Tab, Ctrl and the arrows are full-size keys.
- The terminal grid is measured for the screen, so lines wrap where you see
  them wrap.
- Leaving the app does not end the shell. A long command keeps running, and
  its output is there when you come back.

The shell is provided by the runtime, not the app. Only the Terminal app is
allowed to use it.

## Development

```sh
cargo test -p kobo-terminal
kobo run --sim --app terminal      # in the browser simulator
kobo deploy --device <ip>       # onto a reader over Wi-Fi
```
The simulator starts a real `/bin/sh` on your computer.

---

Part of [Cobalt](../../README.md). See [all apps](../../README.md#apps).
