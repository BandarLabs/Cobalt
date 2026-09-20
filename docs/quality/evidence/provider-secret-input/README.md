# Provider secret input

`kobo readlater login` requires `--client-secret-file PATH`; passing
`--client-secret SECRET` is a usage error. The path must name a non-empty
regular text file no larger than 4 KiB. `--password-file` uses the same bounded
reader, while an environment-variable name remains supported. Post already
requires `--token-file`.

This keeps client secrets, passwords and bearer tokens out of the command
line and ordinary shell history. Three Read Later tests pass, including an
explicit refusal of a direct client-secret value, and strict all-target CLI
Clippy passes.
