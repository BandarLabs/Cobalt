# Sidekick

The permission prompt, moved to the armchair.

A coding agent on the desk stops to ask "may I run this?" and the asking goes
wherever this reader is. The [sidekick daemon](../../crates/kobo-sidekickd)
on the computer catches the question through the agent's own hook system --
Claude Code's `PreToolUse`, Codex's `PermissionRequest` -- and holds it; this
application collects it over one long-polled fetch, prints the command in
full, and offers exactly three answers under a thumb: **Allow**, **Deny**,
and **Leave it for the terminal**.

The design rule is that the panel earns its repaints. An empty poll asks
again without drawing anything; a question repaints once and the panel then
holds it at zero power for as long as the decision takes, which is the one
thing this screen does better than the phone it replaces.

## Setting it up

On the computer, once:

```sh
kobo-sidekickd init                  # certificate, pairing code, address
kobo trust set sidekick --device IP  # the reader learns to verify the daemon
kobo-sidekickd setup codex           # prints the hook config; also: claude
kobo-sidekickd run
```

On the reader, open Sidekick and type the two things `init` printed: the
address, then the six-character pairing code. Both are remembered; pairing is
typed once.

## What the screens hold

Three screens after pairing, and nothing on any of them that is not needed:

- **Watching** -- a splash saying questions will appear, who the reader is
  paired with, and the last answer given, so a glance says the tap counted.
- **Asking** -- who asks, the tool, the command as a quote, three buttons.
  Back is not an escape hatch here: dismissing a question sends "leave it
  for the terminal", said out loud rather than left dangling on the daemon.
- **Sending** -- stated once with `activity`, because a tap with no visible
  answer on a slow panel reads as a tap that was missed.

## Trust, stated plainly

The connection is TLS against a root the owner installed with `kobo trust
set`; the runtime verifies the daemon exactly as it verifies any public
host. The pairing code rides every request so nobody else on the network can
watch the questions or answer them. And the failure mode is honest: when the
daemon is unreachable, the agents' own terminal prompts work exactly as they
did before this application existed.
