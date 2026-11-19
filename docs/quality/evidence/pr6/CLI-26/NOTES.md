# CLI-26: create required storage within validated import

The send engine validates before it allocates: a file is read and checked
first, and the companion's storage on the target is created only once the
content is known good. The transcript shows both halves for real against
the live simulator:

1. The rss shelf is deleted, then a corrupt OPML is sent: the refusal names
   the problem, the selection is kept for retry, and the rss shelf is NOT
   created - a failed import leaves no half-made storage behind.
2. A valid OPML is then sent: validation passes, the shelf is created, and
   the list is staged inside it (the atomic publish of CLI-18).

No code change was needed; this item is evidenced by real simulator runs.
The same ordering holds for the flashcards import path by construction -
`kobo flashcards import` delegates to the signed flashcards-import helper,
which is not distributed in this sandbox, so that path is not exercised
here (labeled, not silently skipped).
