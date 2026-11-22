# Tailscale

Put your Kobo on your [Tailscale](https://tailscale.com) tailnet, the private
network between your own devices. Your computer then reaches the reader from
any network, so `kobo send` and `kobo clip` work away from home, and apps on
the reader can reach your own servers by name.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/tailscale-sign-in.png" alt="The approval code to scan with a phone"><br>The approval code to scan with a phone</td>
<td width="50%" valign="top"><img width="300" src="screenshots/tailscale-connected.png" alt="Connected, with the reader's tailnet name and address"><br>Connected, with the reader's tailnet name and address</td>
</tr>
</table>

One reader has run this as far as the approval code: a Clara BW on firmware
4.46.23852. [First run on a reader](#first-run-on-a-reader) says what that
showed and what nobody has seen yet. The rest has been tested against
stand-ins for the Tailscale programs. The evidence bar in
[docs/DEVICES.md](../../docs/DEVICES.md) applies before it is called tested.

## Features

- One switch. Tailscale starts when the switch is on and Cobalt is open on
  Wi-Fi, and stops when you leave Cobalt.
- Sign in by scanning a code on the reader with your phone. The address is
  written out under the code for a device that cannot scan.
- Once connected, the screen shows the reader's tailnet name and address, and
  a mark appears in the status band on every screen.
- When it cannot start, the screen says why and what to do: no Wi-Fi, a
  download that did not finish, sign-in that timed out, or a reader that is
  not safe to put on a tailnet yet.
- **Sign out** forgets the reader's key. Use it before you give the reader
  away.
- Three options, each off until you turn it on: stay connected outside
  Cobalt, offer the reader as an exit node, and share the network the reader
  is on. See [Options](#options).

## Setup

Everything up to the last step happens on the reader and your phone.

1. On the reader, open Tailscale and turn on the switch. The first time, the
   reader downloads Tailscale over Wi-Fi, about 37 MB, and checks it before
   running it.

2. Scan the code on the reader with your phone, sign in to Tailscale and
   approve the reader. The screen then shows the reader's tailnet name and
   address.

3. Save that address on your computer. After this, `kobo send` and `kobo clip`
   reach the reader from any network while Cobalt is open on it.

   ```sh
   kobo pair kobo --device 100.64.0.7
   ```

In the Tailscale admin console, consider turning off key expiry for the
reader. Keys expire after 180 days by default, and the reader then asks to be
approved again.

## Options

Open **Options** in the app. Each switch is off until you turn it on, and
each says under it where it stands.

### Stay connected outside Cobalt

Keeps the reader on your tailnet after you leave Cobalt, so `kobo send`
reaches it while you are reading in the Kobo software.

What it costs:

- **Battery.** The Tailscale programs keep running and keep a little traffic
  going.
- **Only while the reader is awake.** The Kobo software switches Wi-Fi off
  when the reader sleeps, and the reader cannot be reached until it wakes.
- **Room in the reader's system storage.** The programs and the runtime that
  watches them have to run from there, because the book partition is handed
  to a computer when you plug in USB and nothing can keep running from it. A
  reader without 110 MB free says so under the switch and runs Tailscale
  while Cobalt is open instead.
- **Nothing starts it when the reader restarts.** Open Cobalt once and it
  connects again.

Plugging the reader into a computer does not stop it.

### Offer as an exit node

Lets your other devices send their internet traffic through this reader, so
they appear to be at home.

### Share this network

Lets your tailnet reach the other devices on the Wi-Fi network the reader is
on, such as a printer or a router's settings page. The network is read from
the reader's Wi-Fi at the time and shown under the switch. Only a private
home network of ordinary size is shared. On any other Wi-Fi the switch says
it is not available there.

### What the last two need, and their limits

- **Approval, once.** In the Tailscale admin console, open this reader's
  route settings and allow the exit node or the route. Until then the switch
  says it is waiting.
- **A reader is slow at this.** It has one core and Wi-Fi. Expect a few
  megabits, not a router's speed.
- **The reader stays awake for it while Cobalt is open.** Cobalt normally
  closes itself after an hour untouched, and the Kobo software then puts the
  reader to sleep. Once your tailnet has approved the exit node or the
  network, Cobalt stays open instead, on a charger or down to 20% battery.
  Below that the reader is let go to sleep.
- **It stops when the reader sleeps or Tailscale stops.** The power button
  and the cover put the reader to sleep as always. So does the Kobo
  software's own sleep timer once you have left Cobalt, with Stay connected
  on or off. A phone using the reader as its exit node loses its connection
  until you pick another.
- **TCP, UDP and ping only.** The Tailscale program carries each connection
  itself and makes it again from the reader.
- **Unverified on any reader.** If the reader turns out not to be able to
  carry traffic, the switch says it is not available and nothing is offered
  to your tailnet.

## Reaching your own servers from apps

Any app that may use the network can reach a computer on your tailnet, by
its tailnet name or its address. The app does not have to know about
Tailscale: a library in calibre-web, a Miniflux or Readeck server, a home
panel.

Apps only make `https` requests, on your tailnet as everywhere else, and most
things people run at home answer plain `http` on a port. Give the one you
want a certificate. On the computer that runs it:

1. In the Tailscale admin console, under **DNS**, turn on MagicDNS and HTTPS
   certificates.
2. Run `tailscale serve --bg 8083`, with the port your server listens on in
   place of 8083. It prints the `https` address to use, which looks like
   `https://nas.example.ts.net`.

Type that address where the app asks for your server. The reader knows your
tailnet's names while Tailscale is running, and the certificate is one every
app already trusts, so nothing is installed on the reader.

This reaches the computers on your tailnet and nothing behind them. The
reader does not use a network another device shares, and does not send its
own traffic through an exit node.

## What the reader opens to your tailnet

The reader has no packet filter, so every port it listens on is reachable
from the tailnet once it joins. A tailnet is every device signed in to it and
anything shared into it, not only your laptop.

The runtime will not join a tailnet, and leaves one within a minute, unless
it can account for every TCP port the reader is listening on. What it
accepts:

- **SSH, on the ports the SSH server says are its own.** This is the point:
  it is how `kobo` reaches the reader. The server is asked what it would
  enforce for root arriving from a tailnet address, and must refuse root
  anything typed: password login off, and question-and-answer login off
  wherever the server uses PAM. `kobo setup` arranges this, and the reader
  arranges it for itself when it has not been done: see
  [Password login](#password-login). A server that cannot be asked counts as
  open.
- **Sync, while a sync is running**, recognised by the Sync engine being the
  program that holds the port. It accepts only the computers you paired.
- **Tailscale's own listener** on the tailnet address.

Anything else refuses the join: a second SSH server, telnet or FTP from the
reader's debug services, something started from a shell. The screen names
the port and, where it can tell, the program holding it. So does a reader
that cannot say what is listening. Nothing from Cobalt itself listens on a
network port.

If you run a program of your own on the reader, have it listen on
`127.0.0.1` and reach it through SSH, which the tailnet already carries:

```sh
ssh -L 8080:127.0.0.1:8080 root@kobo-cobalt
```

A program listening only there is not offered to the tailnet and does not
stop the reader joining. If a reader you have added nothing to is refused,
the program named is part of its firmware and the list above is one short:
report the port and the name.

### Password login

The Kobo software's SSH server is off unless something turns it on, and
`kobo setup` is what usually does. As Kobo ships it, that server lets root in
with a password, including an empty one. `kobo setup` closes that once its
key is in place. A reader set up before setup did so is still open.

The first time you switch Tailscale on, the reader closes it for you, if a
computer's key is already on the reader so that you are not shut out. It
adds the same marked block `kobo setup` writes to `/etc/ssh/sshd_config`,
between `# cobalt-hardening: key-only SSH for root.` and
`# cobalt-hardening: end.`, has the server check the file before using it,
restarts the server, and puts the file back if the server does not then
refuse root a password. Every `kobo` command logs in with the key, so they
work as before. Logging in to the reader by typing a password stops working,
from your Wi-Fi as well as from the tailnet.

Switching Tailscale off or signing out leaves the block where it is. To have
password login back, delete those lines over SSH and restart the reader.

A reader with the server on and no key the server would accept is left
alone, because a password is then the only way in. So is one whose server
was started with settings of its own, which its file does not decide.
Tailscale does not start there, and says to run `kobo setup`.

The reader can see that a key is there and that the server would take it.
It cannot see whether you still have the other half. If you do not, logging
in over SSH stops working until `kobo setup` is run again over USB, which
needs no login.

### What this does not cover

This covers TCP. A program that only receives datagrams cannot be held to a
list this way and stays reachable from the tailnet. On a stock reader that is
the Wi-Fi address client, Tailscale itself, and Sync's discovery while a sync
runs.

Unless you switch an option on, the reader is a leaf. It forwards no traffic,
offers no exit node and no routes, accepts none, and does not run Tailscale
SSH. Its DNS is left alone:
the names of your tailnet's computers are kept in a marked block in
`/etc/hosts` while Tailscale is running, and taken out when it stops.

## What it keeps, and where

- **The reader's key** is in `/usr/local/cobalt/tailscale` on the reader's own
  system partition. It is not on the partition a computer sees over USB.
- **It survives** a restart, a Cobalt update and reinstalling Cobalt, so the
  reader stays the same device on your tailnet and needs no second approval.
- **A factory reset erases that partition**, and the key with it. The reader
  then asks to be approved again and appears as a new device, usually named
  `kobo-cobalt-1`. Remove the old one in the admin console. A firmware
  update writes only its own files, so the key is expected to outlive one;
  that has not been watched happen.
- **Uninstalling Cobalt does not remove the key**, because uninstalling
  happens over USB and cannot reach that partition. Use **Sign out** in the
  app first. If the reader is offline when you sign out, also remove it in
  the admin console.
- **The two Tailscale programs**, about 70 MB, are downloaded to the book
  partition beside the Sync engine, and downloaded again after each Cobalt
  update.
- **They are run from the reader's system storage**, in
  `/usr/local/cobalt/tailscale-bin`, when it has 110 MB free. The book
  partition can be rewritten by any computer the reader is plugged into, so
  the programs are copied from it, checked against pinned digests where they
  land, and checked again before every start. Switching Tailscale off, or
  signing out, removes that copy.
- **On a reader without that room they are run from the book partition**,
  checked the same way before every start. The difference is that something
  able to write the book partition while Cobalt is open could change a
  program after it was checked. A computer cannot, because the partition is
  not handed over while Cobalt has the screen, and an app cannot, because
  apps are kept out of that folder. Only something already running as root on
  the reader could.
- **With Stay connected outside Cobalt on**, a copy of the Cobalt runtime
  that supervises them is in the same folder.

## How it runs

The app saves one word, on or off. The runtime does the rest
(`crates/kobod/src/tailscale.rs`):

1. Checks that the kernel has a tunnel driver and that everything listening
   on the reader is something it can account for, closing password login
   for root first where a key is there to keep you in.
2. Downloads the pinned build, `tailscale_1.104.7_arm.tgz`, from
   `pkgs.tailscale.com` if it is not installed, four megabytes at a time
   straight to a file, and refuses anything that does not match the digests
   compiled into the runtime.
3. Starts `tailscaled` with its state under `/usr/local/cobalt/tailscale`, log
   upload and file transfer off, and a memory limit sized for a reader.
4. Runs `tailscale up` with every leaf setting named and anything set by hand
   reset.
5. Stops the daemon when you switch Tailscale off, and before the reader is
   handed back to the Kobo software when you leave Cobalt.

Unless you asked it to stay connected, Tailscale does not run while the Kobo
software has the screen. It never runs while the reader is asleep. Open
Cobalt to bring it back; a reader that has been approved reconnects in a few
seconds.

With **Stay connected outside Cobalt** on, step 5 changes. The runtime copies
itself and the programs to the reader's system storage and starts the same
supervisor from there, detached, so it outlives Cobalt. It goes on checking
what the reader listens on and stops the daemon if that becomes unsafe. While
a computer has the book partition over USB it reads nothing from it and
writes nothing to it. The next time Cobalt opens it finds that supervisor
and uses it, or replaces it if Cobalt has been updated since.

### How the exit node and the shared network are carried

A Linux computer that routes for a tailnet has its kernel forward and
translate the packets, which Tailscale sets up through the kernel's packet
filter. A Kobo's firmware ships no packet filter tools, and whether its
kernel has the filter at all is unknown. So the reader does not do it that
way. The daemon is started with `TS_DEBUG_NETSTACK_SUBNETS=true`, which makes
it carry routed connections itself while the reader's own address stays on
the kernel's tunnel. Nothing in the kernel is changed: no forwarding switch,
no firewall rule.

Two things about that setting are worth knowing. It is not documented by
Tailscale; it is the setting their program uses by default on network storage
boxes, which lack the same things. And the daemon's other way of working
without the kernel, its fully userspace mode, is deliberately not used: it
passes every connection for the reader itself to `127.0.0.1`, which would put
ports this runtime keeps private on your tailnet.

Because the setting is undocumented, the runtime checks that it took. A
daemon that is still relying on the kernel says that IP forwarding is
disabled when it is asked to advertise anything on a reader. If that is seen,
the offer is withdrawn and the switch says it is not available. When the
pinned Tailscale version is changed, this is one of the things to check
again, with the digests.

Files sent to the reader with Taildrop are not accepted.

## First run on a reader

A Clara BW (N365, firmware 4.46.23852, kernel 4.9.77) has run this as far as
the approval code. What that showed:

- The kernel has the tunnel driver, and the daemon brought `tailscale0` up
  on it.
- The reader closed password login for root itself, and logging in with the
  key went on working, through the server's restart.
- The download arrived and matched its digests, and the programs ran from
  system storage, which had 719 MB free.
- With the programs already downloaded, the approval code was ready about
  fifteen seconds after Cobalt opened.
- The daemon held 30 MB while it waited for approval, with its
  `oom_score_adj` at 800 and the default route still on Wi-Fi.
- Leaving Cobalt stopped the daemon and removed its interface, and Wi-Fi
  went on working in the Kobo software.
- A program left listening on a port was refused by name, once.

Nobody has yet seen, on a reader: approval and the connected screen, traffic
over the tailnet, staying connected outside Cobalt, the exit node, the shared
network, or the key outliving a restart. So a run that gets further is
evidence as well as a test. With the reader at `READER` on your Wi-Fi,
capture these before switching Tailscale on, once it says **Connected**, and
after leaving Cobalt:

```sh
kobo doctor --device READER
ssh root@READER 'grep -E "MemTotal|MemAvailable" /proc/meminfo; grep tun /proc/misc; cat /etc/hosts; ip route'
ssh root@READER 'pid=$(pidof tailscaled); grep -E "VmRSS|VmHWM" /proc/$pid/status; cat /proc/$pid/oom_score_adj'
ssh root@READER 'cat /mnt/onboard/.adds/cobalt/state/tailscale/tailscale-status; tail -40 /usr/local/cobalt/tailscale/tailscaled.log'
```

With an option on, also capture:

```sh
ssh root@READER 'ls -la /usr/local/cobalt/tailscale-bin; df -h /; tr "\0" " " < /proc/$(cat /usr/local/cobalt/tailscale/supervisor.pid)/cmdline'
ssh root@READER 'tr "\0" "\n" < /proc/$(pidof tailscaled)/environ | grep -E "NETSTACK|GOMEMLIMIT"; cat /proc/sys/net/ipv4/ip_forward'
```

Staying connected is working when the supervisor's command ends in
`--tailscale keep`, `tailscaled` is still running after you leave Cobalt and
after you plug the reader into a computer, and Wi-Fi still works in the Kobo
software. The exit node and the shared network are working when the daemon
has `TS_DEBUG_NETSTACK_SUBNETS=true`, the forwarding switch still reads `0`,
and a phone on mobile data that picks the reader as its exit node shows your
home connection's address.

What a good run looks like: the daemon's memory is in the tens of megabytes,
its `oom_score_adj` is 800, the default route is still on the Wi-Fi interface,
`/etc/hosts` has one marked block while connected and is byte for byte what
it was once you have left Cobalt, and no `tailscaled` is running then. Stop
and report if Wi-Fi does not come back in the Kobo software after you leave
Cobalt, or if the screen says **Not supported**: that reader's kernel has no
tunnel driver. `tailscaled.log` is the daemon's last 200 lines and is what a
report needs. It can name your tailnet's computers, so read it before you
share it.

## Development

```sh
cargo test -p kobo-tailscale
cargo test -p kobod tailscale
python3 scripts/check-apps-sim.py tailscale --all-routes
```

The first runs the app's own tests. The second runs the supervisor against
two shell scripts standing in for the Tailscale programs, which is every
state and failure that can be reached without a reader. The third opens the
app in fresh simulators and plays `drive.kobo` and each `drive-*.kobo`.

## Credits

[Tailscale](https://tailscale.com) is licensed under the BSD 3-Clause
License. This app is unofficial and not affiliated with or endorsed by
Tailscale Inc. See [THIRD-PARTY.md](THIRD-PARTY.md).
