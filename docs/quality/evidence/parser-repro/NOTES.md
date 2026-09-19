# Parser real-story reproduction (Zork 1)

Probe: push the real zork1.z3 (sha256 37084966477dff679282de42974b2077156b1bd68fad92a65d4ea94d8eb64d79) into a seeded simulator, open it from the library, screenshot the story screen.

Finding 1: the branch-4 parser (TRANSCRIPT_PAGE_BYTES split) refuses the story screen - "node 2: text does not fit its rectangle" - because a byte-budget page does not fit its panel. The branch-3 measured pagination does not have the bug; the probe passes at default and extra-large on clara-bw-391.

Finding 2 (fixed here): the save/restore list added the story checkpoint row beside the three paged slot rows, so every restore page carried one row over budget. At the largest text scale the guidance line lost its room and the renderer refused the screen ("node 2: text does not fit its rectangle"). The checkpoint is now paged with the slots instead. Covered by slot_rows_name_what_each_slot_holds with the real measuring face installed (the fallback face had masked it) and by play_screen_status_fits_every_panel_and_text_size across three panel geometries x four text scales x keyboard states.

Shots: default-zork-open.png (2 of 2), extra-large-zork-open.png (3 of 4, bar title ellipsised).
