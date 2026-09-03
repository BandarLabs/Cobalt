# Sync

Sync is the Settings surface for Cobalt's runtime-owned Syncthing service. It
persists the on/off state and cadence, reads runtime-written transfer status,
and shows the fixed receive-only `vault`, `frame`, and `books` folders plus
send-only `out`.

![Sync folders on the Clara BW simulator](screenshots/syncthing-folders.png)

`kobod --syncthing window 300` starts a bounded owner-attended window;
`tail` limits opportunistic windows to 90 seconds, `scheduled` honours the
stored cadence, and `status` reports the latest runtime status. The supervisor
generates configuration in `/var/lib/cobalt/syncthing`, makes a daemon-only
API key from `/dev/urandom`, and only permits its REST listener on loopback.
It accepts neither arbitrary folder paths nor a key supplied by an app. A
clean REST shutdown is attempted before a last-resort kill.

The unmodified ARMv7 Syncthing binary and a persistent scheduled-wake launcher
are still required on an actual Kobo platform image. Until the binary arrives
through the platform-update channel, the app surfaces the install remedy.

## Pair one host directory

The host CLI creates a separate Syncthing identity and home at
`~/.config/kobo/syncthing`; it never edits another Syncthing installation.
Install Syncthing with the computer's package manager, wake the Kobo on Wi-Fi,
then map one real directory to one fixed Kobo folder:

```sh
kobo sync setup ~/Documents/notes --folder vault --device 192.168.1.2
kobo sync run
kobo sync status
kobo sync stop
```

`vault`, `frame`, and `books` are send-only on the host and receive-only on the
Kobo. `out` is the inverse, and its host directory must be empty at first
setup. Setup obtains both real Syncthing device IDs, adds only the chosen
folder to the pairing, rejects symlinked or overlapping roots, and prints the
attended Kobo window command. `run` starts only the dedicated peer in the
background; `run --foreground --seconds 300` provides a bounded alternative.
The API remains on `127.0.0.1:8385`, while LAN discovery, global discovery,
and relays remain enabled for direct and remote transfers.

## ARMv7 release path

`build-armv7.sh` is deliberately build-only: it requires a locally reviewed,
pinned upstream checkout and a caller-provided output directory. It does not
download or execute a release artifact. It pins Syncthing `v2.0.9` commit
`3382ccc3f16536b5a7b6df7c8212951f7d4d3a9f`, runs `go mod verify`, and refuses
an output that differs from the repository-pinned SHA-256. Set
`COBALT_SYNCTHING_ARTIFACT` to that verified binary before packaging. `kobod`
compares the installed engine to the same digest compiled into the runtime; it
refuses a symlink, group/world-writable, non-root-owned, or mismatched engine
before spawning it.

Choosing hourly, four-hourly, or daily in the app requests Cobalt's bounded
scheduled-wake facility; manual mode and Pause cancel it. A scheduled launcher
must run `kobod --syncthing scheduled`, which checks the persisted setting
again immediately before opening a five-minute maximum window.

## Third-party attribution

[Syncthing](https://github.com/syncthing/syncthing) is MPL-2.0. The shipped
binary's exact upstream version and commit must be recorded in the platform's
`THIRD-PARTY.md` when the platform package selects it.
