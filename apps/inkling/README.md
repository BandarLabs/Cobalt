# Inkling

An offline daily five-letter puzzle. The answer is `answers[djb2(YYYY-MM-DD ++ salt) % len]`, so
one shipped build produces the same daily game everywhere without a network service. Shape states
are grayscale-first: `[letter]` is placed, `(letter)` is present, and `letter×` is absent.

![A solved Inkling puzzle on Clara BW](screenshots/inkling-solved.png)

The compact game includes six guesses, duplicate-correct scoring, hard-mode revealed-letter checks,
saved daily progress, and cumulative played/won statistics. The puzzle day is UTC;
`KOBO_INKLING_DAY=YYYY-MM-DD` pins it for simulator recordings. It ships a deliberately small common-word seed list rather than any copied
commercial answer list. No trademarked game name or source list is used.

## Capabilities

None. Inkling is offline forever.
