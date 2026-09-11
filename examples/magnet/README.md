# Magnet Sensor

Where the magnet is, and nothing else.

The reader has a hall sensor behind one edge of the bezel. It is the thing a
sleep cover closes against. This application shows everything the SDK exposes:
ask once for the state, then wait to be told when it changes.

| Sweeping an edge | The edge that answered |
| --- | --- |
| ![The reader drawn with the top edge marked as the one to sweep](screenshots/no-magnet.png) | ![The same diagram with a ring where the sensor answered](screenshots/counting.png) |

*Captured from the Clara BW simulator by the committed route, which uses the
simulator's own hall-sensor controls. The device capture is taken with the
hardware acceptance run.*

## Using it

The reader is drawn with one edge marked: that is the edge to sweep. Hold a
magnet against it and move it slowly along. The moment the sensor answers, the
screen says so and a ring appears on that edge of the diagram. **Sweep the
next edge** walks round the four of them.

Nothing here claims to know where the sensor is. The profiles describe the
panel, not the magnet, and the bezel says nothing. What the application does is
help you find it and then write down which edge answered, so opening it again
on the same reader starts at that edge and says "Found on the right edge"
rather than asking you to do it all over.

## Why there is a count

A magnet moved slowly past the threshold can bounce, and a run that reads six
changes where your hand made one is telling you something a gesture built on
this sensor needs to know before it is written. The count resets from the
screen, because the number is only useful against the sweep you have just done,
and it is counted per edge: how many times it moved matters less than where it
was when it did.

It counts movement, not answers. The first reading establishes the state rather
than changing it, and a restated state is not an edge.

## What the SDK gives you

```rust
fn on_start(&mut self, context: &mut Context) {
    context.device().read_cover();
}

fn on_cover_change(&mut self, context: &mut Context, magnet_present: bool) {
    self.present = magnet_present;
    context.set_screen(self.screen());
}
```

Two facts about the hardware leak through, because pretending otherwise would
produce applications that are quietly wrong:

- **Edges are not the state.** A magnet already sitting against the bezel when
  this opened produced no event and never will, which is why `read_cover` is
  asked once at the start. It is the same reason the runtime queries the key
  state when it opens the sensor rather than waiting for the first change.
- **Only the foreground application is told.** A magnet arriving is something
  that happened in front of the reader. A backgrounded application has no
  standing to react to it and asks again when it returns.

## What it deliberately does not do

It does not say "cover closed". The sensor cannot tell a cover from a fridge
magnet, and an application that reports the one thing when it measured the
other is inventing a reading. The runtime says what it measured; deciding what
that means is the application's job, and this application declines to.

---

Built with the [Cobalt SDK](../../README.md), which
[installs on a Kobo](../../README.md#install-it-on-your-kobo) with one
command over USB. The other apps:
[Launcher](../launcher/README.md) ·
[Audiobook Studio](../audiobook/README.md) ·
[Gutenbird](../gutenbird/README.md) ·
[Hacker News](../hn/README.md) ·
[RSS Reader](../rss/README.md) ·
[Daily Brief](../brief/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md)
