# kobo-stream

The host implementation behind `kobo stream` owns the command process, a
`vt100` screen model, and the TLS listener. Paperterm devices receive
plain-text changed rows and cursor positions only; no terminal history or
credentials are written to the reader.

```sh
kobo stream init --host 192.168.1.20
kobo trust set stream --device READER_IP
kobo stream --interactive -- /bin/sh
```

The service uses a per-run random session id, requires the six-character
pairing code on every route, limits readers to sixteen connections, limits
input to 64 bytes, and retains the final screen for sixty seconds after the
command exits. Host output is converted through `vt100`; colour attributes are
dropped, box drawing is retained, blocks and braille degrade predictably, and
unknown wide glyphs become `·`.

The command runs behind Cobalt's existing safe PTY wrapper. It has a
controlling terminal at the negotiated grid, so terminal applications such as
`vi` and the shell see the same cursor screen as the reader. Screen
snapshots are deliberately capped at two per second; input writes go directly
to the PTY and do not wait for that display cadence.


In interactive mode, the laptop and Paperterm send input to the same PTY.
Laptop input uses raw terminal mode, restored when the child ends. Output is
flushed even without a newline, so shell prompts appear immediately. Output
draining yields after a bounded batch so a busy command cannot indefinitely
hold the input lock. Paperterm defaults to portrait and negotiates its measured
grid, including when its keyboard opens or closes.

For isolated tests, set `KOBO_STREAM_CONFIG_DIR` to an absolute directory; the
identity lives in its `stream` subdirectory and the trust copy in `trust`.
Without this override the location remains `~/.config/kobo`. The live fixture
in `scripts/quality/check-paperterm-live.py` exercises a real laptop TTY, host
PTY and SDK simulator over trusted TLS without using the owner's identity.
See [the captured session](../../apps/paperterm/screenshots/terminal.png).

## Check the connection before sharing a terminal

After the one-time `stream init` and reader trust setup above, run:

```sh
kobo stream demo
```

Open Paperterm and connect to this computer. Type a short message on the reader
and press Enter; both screens show it. Type another message on the computer to
check the other direction. The check echoes text and never treats it as a shell
command. It is included in the CLI, so no Python, sample project or shell script
is needed. Keep the computer awake and on the same network.

Type `exit` on either screen to finish. The final screen stays available for
one minute, then sharing ends. `--port PORT` is available when the default port
is already in use. Use the existing `-- COMMAND` form only when you want to
share a particular terminal program.

![Reader and laptop messages in the same session](../../apps/paperterm/screenshots/connection-check.png)

To see the saved address and pairing code again, use `kobo stream pairing`.
This reads the existing identity without replacing its keys or code. If you
choose another port, use `kobo stream pairing --port 9123` and
`kobo stream demo --port 9123` so the displayed address matches the service.
The demo also displays these details when it starts. An older setup without
a saved address explains how to add the computer address with `stream init`.
