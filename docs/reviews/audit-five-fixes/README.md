# Archived review evidence

[Browse the original images, diagnostics, provenance, and reproduction notes](https://github.com/BandarLabs/Cobalt/tree/e0eda8264d57111aaae2fc3a0bab0f5a17d4fbf8/docs/reviews/comment-remediation-20261002/pr-273/docs/reviews/audit-five-fixes). The immutable archive preserves this review packet byte for byte; it is historical evidence for its recorded source revisions, not proof for later edits.

Reusable capture drivers now live in [tools/review-captures](../../../tools/review-captures/README.md), outside production app sources. Their commands use relocated script/scenario paths; ordinary regression tests remain with the app.

Runtime provenance correction: the 100% record identifies `ec0a97e3`; the 170% record identifies `56437121`. Both used the same CLI hash, built from integration source `dcf80eeb`. These are separate recorded worktree revisions, not one shared source-commit field. Physical hardware was not tested.
