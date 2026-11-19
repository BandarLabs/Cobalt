# Frame sleep ownership and energy validation

## What the simulator proves

`scripts/quality/check-frame-power-sim.py` drives the real app against the
single-app simulator with the power backends enabled
(`KOBO_SIM_BACKENDS=...,keep-awake,scheduled-wake`). The simulator reports
wake ownership on `GET /power` and delivers a scheduled wake when the manual
clock crosses it. Evidence lives in `docs/quality/evidence/frame-power/`.

Verified at the recorded source revision:

- Frame mode holds the wake lock for exactly the interval plus one minute
  (15-minute interval: 960 s hold) and schedules nothing.
- Slow slideshow releases the hold and schedules a real wake exactly one
  interval out (6-hour interval: 21,600,000 ms).
- A clock crossing fires the wake: the app advances to the next photo and
  schedules the next wake one interval later, with no wake hold.
- Switching back to Frame mode cancels the scheduled wake and re-arms the
  hold.
- A restart restores the saved mode and position and re-applies the matching
  power policy.

## What needs the physical reader

Energy numbers are not claimed from the simulator. On the Clara BW, follow
the power method in `clara-bw-validation.md`: an external current logger or
a verified charge counter, with the method and sampling interval recorded,
and windows long enough to hold complete cycles. Battery percentage alone is
too coarse.

Windows to compare, each against the same panel image:

1. Idle sleep with no app scheduled (baseline).
2. Slow slideshow at the 1-hour interval: the reader sleeps between changes;
   one wake per hour, one photo advance, return to sleep.
3. Frame mode at the 15-minute interval: the reader stays awake; the cost is
   the hold plus one full-panel refresh per interval.

The expected outcome to test, not assume: slow slideshow's hourly wake window
sits within measurement noise of baseline, and frame mode's draw is dominated
by the GC16 transition, not the hold itself. Also confirm on hardware that a
scheduled wake survives a real suspend and that the wake-up photo advance
completes before the reader may sleep again.
