# kobo-stream

The host implementation behind `kobo stream` owns the command process, a
`vt100` screen model, and the TLS listener. Paperterm devices receive
plain-text changed rows and cursor positions only; no terminal history or
credentials are written to the reader.

```sh
kobo stream init --host 192.168.1.20
kobo trust set stream --device READER_IP
kobo stream --controls -- claude
```

The service uses a per-run random session id, requires the six-character
pairing code on every route, limits readers to sixteen connections, limits
input to 64 bytes, and retains the final screen for sixty seconds after the
command exits. Host output is converted through `vt100`; colour attributes are
dropped, box drawing is retained, blocks and braille degrade predictably, and
unknown wide glyphs become `·`.

The command runs behind Cobalt's existing safe PTY wrapper. It has a
controlling terminal at the negotiated grid, so terminal applications such as
`vi` and Claude Code see the same cursor screen as the reader. Screen
snapshots are deliberately capped at two per second; input writes go directly
to the PTY and do not wait for that display cadence.
