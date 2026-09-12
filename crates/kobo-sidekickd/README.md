# kobo-sidekickd

The computer companion for [Sidekick](../../examples/sidekick) and
[Deck](../../apps/deck). It relays coding-agent questions to a Kobo and serves
the fixed command controls the owner puts in `deck.toml`.

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
kobo-sidekickd setup          # choose one integration by number
kobo-sidekickd setup claude --dry-run  # preview one integration
kobo-sidekickd setup claude --print    # print configuration for manual setup
kobo-sidekickd setup claude            # configure the named integration
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

### Choosing an integration

`setup` displays supported integrations, whether each was detected, and its
configuration path. Choose one number to configure that integration, or press
Enter or **0** to cancel. It no longer configures every detected integration
at once. When input or output is redirected, it lists status and explicit
commands without changing configuration.

`setup AGENT --dry-run` previews the change. `setup --dry-run` retains the
preview of all detected integrations. `setup AGENT --print` prints manual
configuration. Explicit setup merges the selected hooks with the existing configuration.
Before replacement it saves the previous file as `.json.bak`, then
`.json.bak.1`, `.json.bak.2` and so on, without overwriting earlier backups.
New configuration is written and synced to a staging file before publication.
An occupied staging file or invalid existing JSON stops setup and preserves
the original. Repeating an already completed setup creates no extra backup. Help (`--help`, `-h`, or `help`) returns success; subcommand help is
also available without initializing pairing, starting listeners or installing
hooks.

## The two listeners

Deliberately different, because their trust is different:

- `127.0.0.1:9330`, plaintext, for hooks on the same machine. `POST /ask`
  blocks until the question is decided or five minutes pass, because the
  hook protocol is "write your decision to stdout before you exit".
- `0.0.0.0:9331`, TLS, for the reader. `GET /pending` long-polls up to
  twenty-five seconds for a question; `POST /answer` delivers the tap. Deck
  uses `GET /deck`, `POST /deck/press`, and `GET /deck/result` on the same
  listener. Every reader route demands the pairing code.

The TLS server side lives in `kobo-net::serve`, beside the client it was
built to talk to, so the workspace's network dependencies stay in one crate.

## Deck command safety

Deck routes are unavailable unless `~/.config/kobo/sidekick/deck.toml` exists.
The reader can choose only commands already present in that computer-owned
file. Confirmed keys require a second explicit press, stale key IDs cannot run a
changed command, one key cannot overlap itself, and no more than four commands
run at once. Commands receive ten minutes, then their whole process group gets
`SIGTERM` followed by `SIGKILL` after ten seconds. The daemon strips terminal
escape sequences and retains only the last 2 KB of output.

## Try a sample

Run `kobo-sidekickd init` once to create pairing, then stop any running
Sidekick daemon and run `kobo-sidekickd sample`. Open Sidekick on your paired
reader and choose **Received**. The computer reports when the acknowledgement
arrives. Press Ctrl-C to stop; run the sample again to repeat it.

The sample supplies its own question, opens only the authenticated reader
listener, and does not load Deck or connect to an agent. It runs no command
when you answer. An unanswered question expires after five minutes. Pairing
and certificate installation are the same as normal Sidekick use.

For isolated local testing, `KOBO_SIDEKICK_CONFIG_DIR` overrides the root
containing `sidekick/` identity and `trust/` certificates. It does not change
agent integration configuration paths. Leave it unset for normal owner use;
a custom root's certificate must be installed explicitly if pairing a reader.
