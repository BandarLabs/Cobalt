# CLI-25 - consistent verbs and reader/simulator/output target resolution

One module, `targets`, owns the target flags: --sim for the simulator,
--device HOST / -s HOST for a reader by address, --reader NAME for a saved
reader. One parser (any position on the line), one resolver (exactly one
target required; none or several is a usage error), and no silent
first-reader fallback: a saved name that has no reader behind it is a target
error, exit 3. Nickname resolution is the seam for the saved-reader store
(TODO(CLI-05) in targets.rs). `kobo wait` and `kobo doctor` are migrated;
the verb contract (check reads, preview shows, prepare writes, push sends,
status reports) is documented in --help.

Evidence: transcript.txt - real host runs: -s in any position, conflicting
targets refused as usage (exit 2), --reader resolving honestly to a target
error (exit 3), --sim refused where it cannot apply, and the --help contract
excerpt. Tests: targets::tests::*, tests::wait_and_doctor_take_the_shared_target_flags.
