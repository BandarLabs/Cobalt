# CLI-23: notify only useful completion or required action

The CLI's notification surface is its own stdout, and the audit rule applied
is: nothing during quiet work, stage lines only when a stage does real work
the owner can act on, exactly one completion line, and a prompt only when the
owner must choose. The transcript shows the three shapes for real:

1. `kobo send --sim ... --app frame` to the live simulator: preparing line,
   the one change the transfer makes (Add: the photo), the verification
   counts, one completion line ("sent to sim"). No progress chatter.
   Repeating the identical bytes produces one quiet no-op line instead of a
   re-send (the CLI-17 receipt at work).
2. A required-action refusal (`kobo sync run` unconfigured): one line naming
   the fix, nothing else.
3. A misuse (`--app nope`): the error names the valid companions.

No code change was needed: the send stage lines were added under CLI-16 and
the refusals under CLI-10/19 already follow this discipline; this item
audits them against real runs and records the rule. Reader-side notice
policy lives in the companions (apps/*), outside this lane's edit scope.
