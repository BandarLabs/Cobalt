# CLI-37: keep arbitrary commands in explicit advanced controls

Arbitrary device commands live behind exactly one explicit, labeled
advanced surface:

- The guided owner menu (bare `kobo`) offers no shell or raw-command entry
  at all - the transcript lists the whole menu.
- `kobo shell` is now labeled in `kobo help`: "Advanced: run one arbitrary
  command on the reader... Nothing typed here is checked first." Its own
  usage text warns that the command runs as root with no validation and
  points at the named verbs that cover the common reads. (Copy change in
  this batch; the gating behavior is unchanged.)
- The other raw-control verbs carry their constraints in their listings:
  record/doctor/touch-probe are read-only; guard-test needs an explicit
  --confirm. Panel-writing commands (tap, smoke-display) are compiled out
  of this build entirely, as the help footer states.
