# Companion usability protocol

This protocol is ready to run, but has not been performed. Recruit 6-8 people
who have not used Cobalt and do not work on this repository. Include at least
two people who regularly use a screen reader, keyboard-only navigation, high
contrast, enlarged text, or another assistive setup. Record which setup they
choose without recording a diagnosis.

## Setup

Use a fresh host account and a reset simulator for each person. Provide the
signed host installer, one sample photo folder, one original three-card
package, one Markdown-notes folder, one invalid file of each type, and two
simulated named readers. Keep credentials synthetic. The facilitator reads
only: "Use Cobalt to complete each card. Say what you think has happened."
Do not demonstrate commands. Start a timer when the card is handed over.

## Task cards

1. Preview the photos, choose one, send it to the intended reader, and open it.
2. Preview the cards, import them, review one, and retrieve the review log.
3. Prepare the notes, read one offline, then turn optional folder sync on.
4. Prepare a file while the reader is disconnected, reconnect, and finish
   without choosing the file again.
5. With two readers available, send to the named target; then change its
   address and reconnect it without silently selecting the other reader.
6. Recover from one corrupt input, one rejected credential, one missing helper
   and one interrupted transfer.

For the photo, cards and notes tasks ask: "Is it prepared on this computer,
sent to the reader, or available offline? What would you do next?" Record the
answer verbatim before revealing the state.

## Measures

For each task record: completed without help (yes/no), elapsed time, first
useful success time, firmware reboot/setup time separately, number and type of
facilitator assists, wrong-target attempts, repeated file selections, recovery
success, and whether the participant correctly explains prepared/sent/offline.
After every error ask what happened and what they would try; score comprehension
0 (wrong), 1 (partly right), or 2 (right cause and next action).

Success thresholds are set before the run: at least 6/8 complete each primary
flow, at least 7/8 avoid a wrong-target send, median error comprehension >=1.5,
and at least 6/8 recover without selecting the source again. Report every run,
not only successes. An assistive setup is not an assist. Any flow that cannot
be completed keyboard-only or whose status is not announced is a defect even
if the aggregate threshold passes.

## One-week follow-up

Invite the same participants back after 7 days without reminders or a second
demonstration. Repeat one previously completed send, one reconnect and one
recovery. Record completion, time, assists and state comprehension with the
same form. Ask what they remembered, what they searched for, and what they
expected to persist. Compare person-by-person with the first run; do not replace
retention with a fresh-participant average.

## Report

Publish an anonymized table using participant IDs P01-P08. Keep raw notes
private. Separate simulator results, physical-reader results and unperformed
checks. Do not mark a study row complete from this protocol alone.
