# Companion delivery · PR 4

This is the companion portion of the revised four-PR quality plan, based on
beta after #168. It retains all 133 companion and owner-acceptance tasks.
PR #181 continues the remaining catalog apps separately.

The first end-to-end journeys are Paperterm, Frame and Flashcards. They let
an owner demonstrate a live laptop terminal, a personal photo album and a
useful study collection on a reader. A successful demo requires real content,
clear preparation and transfer status, and recovery from a disconnected reader.

1. **Paperterm:** guided start, a harmless connection check, clear waiting and
   connected states, explicit Stop and an explanation that the laptop must stay
   awake. Test bidirectional input with a local fixture terminal. Keep arbitrary
   shell commands in the advanced flow.
2. **Frame:** choose photos, preview crop/pad at reader dimensions, show album
   and storage details, and distinguish prepared files from acknowledged transfer.
   Test corrupt photos, duplicate imports and an unavailable reader without
   losing the prepared album.
3. **Flashcards:** discover the supported helper and its installation status,
   preview a small original deck, verify and transfer it, then export its review
   log. Preserve the separate helper's existing license/distribution boundary.

Each flow needs CLI tests, a driven simulator journey, screenshots and updated
public instructions. Simulator success does not certify physical transfer or
panel behavior. Run the combined Clara BW acceptance after the relevant beta
builds are available. Stable promotion follows that acceptance; this PR does
not promote or merge beta into main.

Remaining companion groups stay in scope. Do not mark a task complete from
this plan alone, and do not report content as available offline until its
installation or import has been acknowledged.
