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
