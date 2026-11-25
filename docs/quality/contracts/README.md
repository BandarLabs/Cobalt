# Quality contracts

Executable contracts that later pull requests reference and test against.
Each contract names the code that implements it and the tests that enforce
it. A contract is frozen: changing one is a dedicated contract PR, never a
rider on a feature PR.

| Contract | Status | Enforcing code/tests |
| --- | --- | --- |
| [Protocol compatibility](protocol-compatibility.md) | frozen | `crates/kobo-protocol`, session-state tests |
| [Release channels](channels.md) | frozen | `crates/kobo-catalog`, Store UI |
| [Capability availability](capability-availability.md) | frozen | `crates/kobo-sdk`, `crates/kobod` |
| [Simulator matrix](simulator-matrix.md) | frozen | `crates/kobo-profile`, `crates/kobo-ui` |
| [Device resource ownership](device-resource-ownership.md) | frozen | `crates/kobo-profile`, `crates/kobo-hal`, `crates/kobo-handoff` |
| [Update graph](update-graph.md) | frozen | `crates/kobod/src/autoupdate.rs`, `crates/kobod/src/update.rs` |
| [Content identity](content-identity.md) | frozen | `crates/kobo-policy` |
| [App quality manifest](app-quality-manifest.md) | frozen | app `kobo.toml` files, Store |
| [Resource budgets](resource-budgets.md) | baseline measured | simulator performance runs |

Status values: `frozen` (referenced PRs may rely on it), `baseline measured`
(numbers recorded, regression percentage not yet set).
