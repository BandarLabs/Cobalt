# Morse

Send a typed message in Morse code with the front light, one letter at a
time.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/sending.png" alt="The letter S filling the screen while it is sent"><br>The letter S filling the screen while it is sent</td>
</tr>
</table>

## Features

- The letter being sent fills the screen, so it can be read from across a
  room.
- The front light flashes at full brightness on each beat and goes dark
  between them.
- An estimate of how long the message will take updates as you type, next to
  **Send**.
- Your previous brightness comes back when the message ends, when you turn the
  light off or when you leave the app.
- The screen changes once per letter, during the pause before it, so the
  letter is readable while it flashes.

## Speed

One Morse unit is one second, the shortest wait the app can schedule. `SOS`
takes 27 seconds and a short sentence takes a few minutes. This also keeps the
flashing well below the rate that can affect people with photosensitive
epilepsy.

## Characters

A to Z, 0 to 9, and `.` `,` `?` `/`. Other characters have no code. They are
left out, and the screen lists which ones.

## Permissions

- `frontlight-control`: flashes the front light.
- `keep-awake`: stops the reader sleeping partway through a message.

## Development

```sh
cargo test -p kobo-morse
python3 scripts/check-apps-sim.py morse
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`.
