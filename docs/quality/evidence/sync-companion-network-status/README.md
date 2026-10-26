# Sync companion network and status evidence

`host-result.json` records seven passing checks. The host-side acceptance uses `scripts/quality/check-sync-cli.py` and its loopback
Syncthing/SSH doubles to exercise the real `kobo sync` process: setup, plan,
background run, structured status, pause, resume, publish, and stop. The updated
run output tells the owner that a background peer uses the computer network,
does not keep a sleeping reader awake, and names status, pause, and quit actions.

The app-side route is `apps/syncthing/drive.kobo`. `app-results.json` records a
passing run in a fresh simulator. `syncthing-on.png` and
`syncthing-folders.png` were inspected at 922x1246: text is legible, controls do
not overlap or clip, the enabled state offers Pause Sync and Refresh status,
and the folders view states its fixed receive/send directions.

This proves the paired owner workflow in the simulator and advances CLI-22 and
CLI-24. It does not prove physical-reader radio, sleep, or power behavior.
