# Contributing

Bug reports, fixes, applications, and device testing are welcome. Open an
issue before a large change. Check open issues and pull requests first, then
comment on the relevant thread with what you plan to change or test.

## Applications

Application contributions have their own guide:
[docs/CONTRIBUTING_APPS.md](docs/CONTRIBUTING_APPS.md).

## Device testing

You do not need to write code to help support another Kobo.

1. Find the matching porting issue linked from the README. If none exists,
   open one before testing.
2. Comment with the exact model, firmware, and whether the device is available
   for attended testing. This also avoids duplicate ports.
3. Begin with the read-only doctor report in
   [docs/PORTING.md](docs/PORTING.md).
4. Run display or touch tests only after a maintainer points you to a reviewed
   candidate profile and commit.
5. Post the command output and observations on the porting issue. The code PR
   should link back to that evidence.

Do not post full serial numbers, SSH keys, credentials, or personal network
details.

Hardware similarity and screenshots are useful context, but they do not make a
profile write-ready. That requires the attended display, touch, exit, and
recovery checks in the porting guide.

## Device port pull requests

Link the porting issue and keep `write_ready` false until the attended evidence
has been reviewed. The pull request should record:

- the exact model, device code, firmware, kernel, and tested commit;
- the doctor report and attended test results;
- the source used for a new controller ABI and tests for its struct layout,
  ioctl values, and waveform mapping;
- any untested hardware revision, firmware, button, stylus, suspend, or driver
  behavior.

Kernel or sandbox fallbacks must fail closed when isolation cannot be applied.

## Testing fixes

For installation or host-platform bugs, include the host OS, Kobo model and
firmware, exact command, complete error, and any confirmed workaround. A
documentation change should describe behavior that has been reproduced, not a
guess.

## Code changes

Keep pull requests focused and explain any behavior or safety boundary they
change. Add tests for changed behavior and run the relevant workspace checks:

```sh
cargo fmt --all --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Report security issues privately as described in [SECURITY.md](SECURITY.md).
