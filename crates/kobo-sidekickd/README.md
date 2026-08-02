# kobo-sidekickd

The daemon half of [Sidekick](../../examples/sidekick): a coding agent's
permission prompt, relayed to a Kobo.

Claude Code and Codex both stop mid-task to ask "may I run this?", and both
have hook systems that let a command answer instead of the keyboard. This
daemon registers as that command, holds each question on a small board, and
serves it to the reader across the room over TLS. The tap comes back and the
hook returns it as if the person had been at the terminal.

Nothing about the agents' setup changes. The hooks are registered once in
their configuration files and fire no matter which frontend asked -- the
Codex CLI and the Codex desktop app run the same core and read the same
hooks. And the failure mode is honest: when this daemon is unreachable or
the reader stays silent, the hook declines to decide, the question falls
through to the terminal prompt it always was, and nothing is worse than
before.

## Commands

```sh
kobo-sidekickd init    # certificate with the LAN address in it, pairing code
kobo-sidekickd run     # both listeners, until killed
kobo-sidekickd setup codex    # prints the hook config to paste; also: claude
kobo-sidekickd hook codex     # what the agent runs; reads stdin, asks, answers
```

`init` writes to `~/.config/kobo/sidekick`: a certificate authority made
once, a leaf certificate minted from it for the machine's current addresses
(add more with `--host`), their keys, and a six-character pairing code. The
authority also lands in `~/.config/kobo/trust`, where the host runtimes
already look, so the simulator trusts the daemon with no further ceremony,
and where `kobo setup` looks, so a reader picks it up with the install. A
reader set up before the authority existed gets it with
`kobo trust set sidekick --device IP`.

## The two listeners

Deliberately different, because their trust is different:

- `127.0.0.1:9330`, plaintext, for hooks on the same machine. `POST /ask`
  blocks until the question is decided or five minutes pass, because the
  hook protocol is "write your decision to stdout before you exit".
- `0.0.0.0:9331`, TLS, for the reader. `GET /pending` long-polls up to
  twenty-five seconds for a question; `POST /answer` delivers the tap. Both
  demand the pairing code.

The TLS server side lives in `kobo-net::serve`, beside the client it was
built to talk to, so the workspace's network dependencies stay in one crate.
