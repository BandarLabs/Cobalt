# Shared command/setup reference check

The default host CLI's public reference was captured from the executable. It
now names every shipped owner companion, including Music Stand, Post, Read
Later, Fieldbook and Panels. Each companion's own generated command reference
is captured beside it.

Fieldbook and Panels setup guides were also rendered from the same bundled
`cobalt-app.json` catalog manifests used for Store publishing. Those guides
therefore inherit the app title, version, capability declarations and setup
steps rather than duplicating them in a second documentation registry.

The default build no longer advertises `present` or `stop`: both dispatch to a
clear compiled-out error unless `device-write` is enabled. A feature-build test
checks that those commands are restored to its conditional help. Tests also
compare the public companion list against the command reference, preventing
new silent omissions.
