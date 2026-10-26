# Morse hardware timing checks

Run these on a real reader before the Morse app ships past beta. The simulator
proves the layout, the order of operations and the beat arithmetic; it cannot
prove that a second on the device is a second on the wrist, that the front
light keeps up, or that the panel and the light agree. Nothing in this
document records hardware results yet.

The beacon's whole timing budget rests on one constraint: the runtime wakes an
application in whole seconds, so a beat is one second and the fastest the
light can ever alternate is 0.5 Hz. That is also the safety margin: the
photosensitive hazard band starts near 3 flashes per second, and no message
can push this beacon anywhere near it. A check that finds the light changing
faster than once a second is a defect, however good it looks.

## Set up

Install the reviewed build, open Morse, and leave the message at its prefilled
SOS. The writing screen states the expected duration: 27 seconds. Have a
stopwatch or a phone clock with seconds ready.

## Timing

- [ ] Send SOS. Time the whole run from the first flash to the last: it ends
      within a second of the estimate on the writing screen.
- [ ] Watch the first letter (S, three dots): three flashes of one second
      each, one second of darkness between them.
- [ ] Watch the second letter (O, three dashes): each flash held three
      seconds.
- [ ] Time the darkness between S and O: three seconds. Where a message has a
      space ("SOS SOS"), the gap at the space is seven seconds.
- [ ] Send the same message twice (Send again on the resting screen). The two
      runs take the same time to within a second: no drift, no catch-up.

## Panel and light together

- [ ] The letter on the panel changes before the light flashes it, never
      after: the first flash of a letter already has that letter on the
      screen.
- [ ] During the darkness between letters the panel shows the letter that is
      coming, not the one that finished.
- [ ] With learning mode on (the chip on the writing screen), the top bar
      names the letter and its code as each is flashed, and the count of
      letters keeps pace with the light.

## Controls under load

- [ ] Tap Stop in the middle of a dash: the light goes out within a second
      and the resting screen appears with the message's codes and estimate.
- [ ] Stop is on the panel for the whole of every letter, including while the
      unsupported-character banner is showing (type a "!" into the message to
      raise it).
- [ ] The device stays awake for a whole message, however long, and sleeps on
      its usual schedule once the beacon has finished or been stopped.

## The light the reader had

- [ ] Set the front light to a known brightness (for example 40%) before
      opening Morse. Send a message to the end. The brightness afterwards is
      the one set beforehand, not the level of the last beat.
- [ ] The same after Stop mid-message, and after leaving the app mid-message
      by backgrounding it: the beacon stops and the light is put back.
- [ ] Toggle Light off mid-send: the light returns to the reader's setting
      while the panel keeps naming letters; toggle it back on and it rejoins
      the beat in flight rather than waiting for the next one.

Record results, with the device model, firmware and build digest, under
`docs/quality/evidence/hardware/`.
