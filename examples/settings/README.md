# Settings

The three pieces of hardware an application cannot reach on its own: the radio,
the Bluetooth chip and the battery.

| Connections | Battery |
| --- | --- |
| ![Bluetooth, Wi-Fi and Battery as rows, each with its state underneath](screenshots/connections.png) | ![A charge bar over eleven facts, from health to charge when new](screenshots/battery.png) |

| Wi-Fi | Bluetooth |
| --- | --- |
| ![Wi-Fi on, one connected network and three more from a scan, each with its signal strength](screenshots/wifi.png) | ![A paired pair of AirPods listed by name and shown as connected](screenshots/bluetooth.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`. The black
bars are network names, painted out by `scripts/redact-ssids.py`: these two
screens are by their nature a list of what the neighbours call their routers,
and that is nobody's business here.*

## Why the front screen states everything

Every row says what it is and what it is doing, on the row. "Bluetooth / Off",
"Wi-Fi / Connected to Fernwood", "Battery / 95% and discharging". A settings
screen whose rows are only nouns makes you open all three to find the one you
wanted, and on a panel that takes most of a second to redraw, that is three
seconds spent learning nothing.

## Battery

The panes are honest about what they measured. Charge, status and time
remaining come from the same sysfs gauge the reader itself uses; capacity,
chemistry, temperature, voltage, current and the three charge figures come from
the fuel gauge and are reported as read. Availability is decided by the
same read that produces the numbers, so the summary and the detail cannot
disagree.

`Read again` exists because current and temperature move while you watch, and a
settings screen that silently goes stale is worse than one that admits it is a
snapshot.

## Bluetooth

Devices are listed by the name the device gave, which is less obvious than it
sounds. `bluez` is reached through `dbus-send`, whose output indents a variant
one level deeper than its parent, so the indentation in front of a string
property depends on how deeply it is nested rather than on a fixed count.

There is no `bluetoothd` on this firmware unless the reader itself has turned
Bluetooth on. Off means off, and the pane says so rather than presenting an
empty list that looks like nothing is nearby.

## Wi-Fi

Turning the radio off is offered, and so is disconnecting, which on a reader
being driven over Wi-Fi ends the session you are using to look at the screen. A
scan is only a scan; it does not join anything.

## What it does not do

No brightness, no time zone, no account, no firmware. Those belong to the
reader and it already has screens for them, and a second set that drifts out of
step with the first is worse than none.

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
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)
