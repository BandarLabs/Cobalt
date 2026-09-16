# Tailscale (experimental)

Tailscale is the Settings surface for Cobalt's runtime-owned Tailscale
service. It persists the on/off state, reads runtime-written status, and shows
the sign-in approval code when the daemon asks for one. It never sees a key, a
peer list, or a flag: the supervisor in `crates/kobod/src/tailscale.rs` owns
all of that.

**P0/P1 experimental: no hardware run has validated this yet.** Host-side review
has verified the pinned archive digest and contents, the daemon/CLI status
protocol, bounded command timeouts, and the parsing/state helpers. Kernel TUN,
startup integration, memory behavior, and suspend/resume remain unverified on
Kobo hardware. The attended evidence bar in docs/DEVICES.md applies before any
of it can be called tested.

## What the runtime does when enabled

`kobod --tailscale ensure` (invoked by the platform wake launcher after the
app schedules a wake, or by hand over SSH):

1. Downloads the pinned upstream ARMv7 build
   (`tailscale_1.102.2_arm.tgz`, digest compiled into kobod) on first use and
   refuses anything that does not match, exactly like the Sync engine.
2. Creates `/dev/net/tun` — `/dev` is devtmpfs, so this happens on every
   start, never at install time — and brings loopback up if the firmware left
   it down.
3. Starts `tailscaled --tun=tailscale0` with state and control socket on the
   root filesystem (`/var/lib/cobalt/tailscale`): `/mnt/onboard` is vfat and
   can hold neither the 0600 state file nor a unix socket. The daemon is
   clamped with `GOMEMLIMIT=48MiB GOGC=50`.
4. Runs `tailscale up --accept-dns=false --netfilter-mode=off
   --hostname=kobo-cobalt`. The Kobo ships no `iptables` binary at all, so
   netfilter mode off is not optional; it is safe because the reader is a leaf
   node. DNS is left stock; instead a marker-delimited block of tailnet peers
   (full `host.tail.ts.net` + short name) is maintained in `/etc/hosts` and
   removed when the service stops.

## Sign-in

The first `up` prints an approval link; the supervisor lifts it into the
status the app reads, and the Sign-in screen shows the short code. Approve
from any browser, then disable key expiry for the device in the admin console:
node keys expire after 180 days by default and the connection would silently
stop.

## Known gaps (tracked for P2+)

- The status-band mark (`kobo_ui::Status`) is not wired yet.
- The platform wake launcher must learn to call `kobod --tailscale ensure`;
  until then, toggles apply when `ensure` is next run by hand.
- Userspace-networking fallback (per-device escape hatch for the Sage-class
  kernel-TUN watchdog crash) is not implemented.
- Per-device `tailscale_tun` profile evidence is pending; the doctor probe
  (`tailscale readiness` section) gathers the facts.
