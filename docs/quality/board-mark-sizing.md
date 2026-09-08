# Fixed board marks and interface size

Sudoku adoption exposed clipping at Large and higher interface sizes: board squares correctly retained their physical geometry, while single-digit labels always used heading size. The shared UI now fits those marks using heading, body or caption, with the same choice in rendering and diagnostics. Keyboard labels retain body size; this is not a general escape from text-overflow checks.

Validation: 266 UI tests pass (two existing ignores). The added regression covers digits, a combining underline, a target square and a note dot at all nine text scales on 212 and 300 ppi panels. It checks the chosen semantic size and matching diagnostics. Actual Sudoku app captures and the complete app journey are recorded on the stacked catalog branch. No physical-reader claim is made.

Nine-column board windows containing complete three-row bands also retain the extra 3×3 separation. This allows overlapping six-row landscape views without changing absolute action IDs or reducing touch targets.

At the smallest legal square, one- or two-character board marks can step down the interface scale if even caption size would clip. Rendering and diagnostics use the same bounded choice. Long labels and ordinary keys do not receive this fallback.
