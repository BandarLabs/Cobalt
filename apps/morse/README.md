# Morse

Sends a typed message in Morse on the reader's front light, a letter at a
time, with the letter currently going out drawn across the whole panel.

<img width="300" src="screenshots/sending.png" alt="The letter S filling the panel while the beacon sends it">

## A beat is a whole second

Morse is defined against a unit — a dot is one, a dash three, the silences one,
three and seven — and every one of those is a duration the application has to
wait out. The only timer this platform offers an application is `Task::Sleep`,
which counts whole seconds, and there is no way to run work off the callback
thread. So the unit here is one second and cannot currently be anything else.

That makes the beacon slow. `SOS` takes twenty-seven seconds; a short sentence
takes minutes. The estimate sits beside the Send key, updating as you type,
because Send starts the beacon there and then and a beacon that quietly
committed you to four minutes would be worse than a slow one that said so.

Slow is also the safe speed. The SDK's own guidance is that flashing the front
light is a photosensitivity hazard, and the hazard band starts around three
flashes a second. A one second unit puts a run of dots at half a hertz.

## The light

On by default, because a beacon nobody can see from across the room is the
feature switched off. Full brightness on a lit beat and dark on an unlit one:
a signal that is merely brighter than the last one is not a signal.

Full here means every bank of the light, which is more than the stock reader
asks for at its own maximum — it sits at one end of the warmth balance, where
half the lamps are dark. The platform lights both at the top of the range and
puts the balance back afterwards, so a lit beat is as much light as this device
can make.

The brightness you had is read when the application opens and put back when it
closes, when you switch the light off, and when you leave for another
application. Most messages end on a dark beat, so without that the reader would
be handed back black.

## The letter

The largest type this platform sets is `FontSize::Heading` at 5.4 mm, which is
a heading and not a signal, and pictures are never enlarged to fit. So the
letter is drawn here at the size it will be seen, out of a five by seven block
alphabet — filled rectangles, no typesetter, and at this size no visible
difference from a real face.

It is rebuilt when the *letter* changes rather than when the light does, which
is what keeps the panel calm: a ten letter message repaints ten times across
two minutes rather than sixty times a minute.

The repaint happens during the silence *before* a letter goes out, not at the
moment its first flash does. E-ink takes the better part of a second to settle,
so a panel repainted on the flash would spend that flash still showing the
letter before it — and the whole point of the screen is that the letter can be
read while the light is sending it. The same rule keeps the gap between words
off the panel: it is a silence rather than a symbol and has no shape to draw.

## What it can send

A to Z, 0 to 9, and `.` `,` `?` `/`. Anything else has no code and is left out,
and the screen says which characters went missing — a beacon that silently
skipped every apostrophe would be transmitting a different message from the one
on the panel.

## Capabilities

`frontlight-control` to flash, and `keep-awake` so a device does not suspend
partway through a message and leave the light wherever the last beat put it.
