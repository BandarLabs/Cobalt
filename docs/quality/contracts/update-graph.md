# Contract: update graph and archive schema

Installation is a graph across three surfaces - first USB/web install,
Store app transactions, platform OTA - not one happy path (issues #154,
#162, #170, #171).

## Archive schema

- Release archives carry a schema version, the updater capabilities they
  require, allowed roots and migration operations.
- An updater refuses an archive schema it does not support before
  download/apply, with a specific recovery path - never a generic
  `InvalidInput` mapped to "address or credentials" copy.
- Error types preserve stage: discovery, DNS, TLS, signature, archive
  policy, disk, migration, activation, launch canary, hand-back.

## Required edges (release CI executes each with the exact old updater)

- latest Stable -> candidate Stable
- previous two Stable releases -> candidate Stable
- latest Beta -> candidate Beta
- Stable -> Beta opt-in -> Stable return
- pre-bootstrap archive layout -> current bootstrap
- installed app on older protocol -> new runtime
- new app requiring newer runtime -> old runtime refusal
- interrupted at every transaction checkpoint -> restart recovery

## App transactions

Commit order: verify -> stage -> atomic activate -> runtime handshake ->
first valid screen -> cleanup of the previous version. Failure at any
point restores the previous app and quarantines the candidate with
diagnostics. Install/update/remove/reinstall preserve unrelated apps,
state, secret references and network-owner configuration.

## Browser installer

The shipped installer JavaScript is exercised through its real registered
event listeners with simulated File System Access handles. The build fails
on referenced-but-undefined helpers or missing registered actions
(issues #170, #171). Installer, updater and uninstall changes ship in
separate PRs unless one atomic migration provably requires otherwise.

## Tests

- Typed stage-error unit tests in `crates/kobod/src/autoupdate.rs` and
  `update.rs`.
- Power-loss injection after each transaction checkpoint in simulator.
- Browser installer E2E fixture in required CI.
