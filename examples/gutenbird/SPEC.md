# Gutenbird as an OPDS client — implementation spec

Working spec. Delete this file once it ships; the shipped rationale belongs in
`//!` headers, the README, and `docs/OPDS.md`.

Read `docs/OPDS.md` first for what the real catalogs do, then
`crates/kobo-opds/SPEC.md` for the model this application consumes.

## What must not change

Gutenbird's interface is the thing being kept. A shelf of covers, a book, a
reader. Everything below is in service of pointing that same interface at any
catalog instead of at one website.

`kobo-read` still owns the reading screen. Type size, front light, bookmarks
and marked passages are not this application's to invent.

**The interface must not reveal which specification a catalog speaks.** No
badge, no "OPDS 2.0" anywhere on the panel, no screen that exists for one
version and not the other, and no wording that changes because the metadata
arrived as Dublin Core rather than schema.org. See `docs/OPDS.md`. In practice
this means one rule with no exceptions: **no drawing code may branch on
`Feed::version`.** If a screen needs to know, the difference belongs inside
`kobo-opds`, not here.

The same holds for what a catalog omits. A catalog that states no language and
one that states English differ by a line, not by a layout; screens are built
from what is present and never reserve space for what is missing.

## What goes away

Every mention of Gutendex. `CATALOGUE`, `CATALOGUE_HOST`, `books_from`,
`next_page`, `total_count`, `whole_number`, `string_array`, `authors_of`,
`plain_text_url`, `cover_url`, and the `?languages=` chip machinery all
described one website's JSON and have no meaning in OPDS. The `Book` struct
becomes a view of `kobo_opds::Publication` rather than a parse of Gutendex.

`LANGUAGES` goes too. A language filter is not something this application
invents any more: OPDS catalogs express it as **facets**, and a catalog that
offers none does not get a filter that silently does nothing.

## Views

| View | What it shows | Where it comes from |
| --- | --- | --- |
| `Catalogs` | The catalogs on offer, and a way to add one | The registry, not the network |
| `Browse` | Rows: somewhere else to go | A feed's `navigation` |
| `Results` | The shelf of covers | A feed's `publications` |
| `Details` | One book, its facts, and how to get it | A `Publication` |
| `Reading` | The book | `kobo-read` |
| `Search` | The keyboard and recent searches | A feed's `search` link |

`Browse` and `Results` are two drawings of one screen, chosen by what the feed
actually holds, not by which catalog it came from. A feed with publications is
a shelf; a feed with only navigation is a list of rows; a feed with both draws
the rows above the shelf, which is what OPDS 2.0 `groups` are for.

## The catalogs

Built in, and none of them is special-cased anywhere but this table:

| Name | Root | Speaks |
| --- | --- | --- |
| Project Gutenberg | `https://www.gutenberg.org/ebooks.opds/` | 1.2 Atom |
| Standard Ebooks | `https://standardebooks.org/feeds/atom/new-releases` | Atom with `enclosure` acquisition |
| Open Library | `https://openlibrary.org/opds` | 2.0 JSON |
| OPDS 2.0 Test Catalog | `https://test.opds.io/2.0/home.json` | 2.0 JSON |

Open Library is the Internet Archive's catalog and the richest 2.0 feed there
is — groups, facets, templated search, a cover on every book. Most of its books
are **borrow-only**: they carry no acquisition link but a `properties.
authenticate` pointing at an `application/opds-authentication+json` document.
Show those as borrow-only and do not offer a download. See `docs/OPDS.md` for
why its open-access links cannot be trusted either.

A reader may add their own by URL. Added catalogs are stored; built-ins are
not, so that changing this table changes what readers see.

**Standard Ebooks' real feeds are behind a donation** and answer `401`. When a
catalog answers `401`, the screen says what the catalog said — that access
needs a Patrons Circle membership — rather than showing a failure the reader
cannot act on. Sending the credential comes later; saying why it is refused
comes now.

## Navigating

A feed stack, because a catalog is a tree and Back means the feed before this
one, not the application before this one. The existing Back behaviour — unwind
the application before leaving it — extends to unwinding the stack first.

Following a link:

1. Fetch it with `Accept` asking for OPDS 2.0 and tolerating 1.x.
2. Parse with `kobo_opds::parse(bytes, url)`.
3. If the feed holds exactly one publication and no navigation, it is a
   **complete catalog entry document** — go straight to `Details`. This is what
   makes Gutenberg work without a single line about Gutenberg: its book entries
   are `subsection` links to entry documents, and this rule turns following one
   into opening a book.
4. Otherwise show `Browse` / `Results`.

`next` is followed only while it stays on the catalog's own host, using
`kobo_opds::same_origin`. The existing "More" behaviour is unchanged.

## Search

See `docs/OPDS.md` for how the two versions differ and why an OPDS 1.2 search
costs an extra round trip. What this application has to do with that:

- A catalog's search template is discovered once and **kept in the registry**
  beside the catalog, so the OpenSearch description document is fetched on the
  first search and never again.
- The Search view shows the keyboard only when the current catalog actually has
  a template. A catalog with no search says so; it does not offer a keyboard
  that does nothing.
- Recent searches are per catalog. A search of Gutenberg is not a search of
  anything else.
- `{searchTerms}` and `{query}` are percent-encoded on the way in. The existing
  test `a_search_term_cannot_add_parameters_of_its_own_to_the_url` still has to
  pass, now against a template rather than a fixed URL.

### Searching several catalogs

A toggle on the Search screen: this catalog, or all of them.

Federated search is **progressive, not parallel**. Catalogs are queued and
asked one at a time; each answer is appended to the shelf as it lands, labelled
with the catalog it came from; covers are not fetched until the queue is empty.
The reason is the four-task ceiling and a slow radio — four searches at once
spend a quarter of a megabyte before the first row appears.

Leaving the screen cancels whatever has not been sent yet, which is why the
queue is held here rather than handed to the runtime all at once.

New tests for this:

- a catalog with no search template offers no keyboard
- a search template is discovered once and reused rather than refetched
- an opensearch description yields the atom url rather than the html one
- a search template on another host is followed, but a paging link on another host is not
- an http search template is upgraded to https rather than refused
- searching every catalog appends each answer as it arrives rather than waiting for all of them
- leaving the search screen stops the catalogs that have not been asked yet

## Covers, and the expensive truth about Gutenberg

Gutenberg **never serves an acquisition feed.** Its search results, its
bookshelves and its author feeds are all navigation feeds whose entries are
`subsection` links carrying a 22×22 `data:` URI icon and nothing else. The
real cover only exists inside the per-book entry document.

Gutendex handed over a title, an author and a cover URL for thirty-two books in
one response. OPDS does not. So:

- When a feed holds **publications**, the shelf draws their covers as it always
  did. Standard Ebooks and the 2.0 catalog take this path.
- When a feed holds **navigation entries**, the rows are drawn immediately from
  what the feed already said, and then the entry document for each book on the
  **visible page only** is fetched to fill in its cover. Six books to a shelf
  page and three cover lanes: the machinery for this already exists and is
  already bounded.

Rows first, covers after, and never fetching the page the reader is not
looking at. A reader who pages fast must not leave twenty requests behind them.

`data:` URI thumbnails are decoded, never fetched. `kobo_opds` hands back the
bytes; the existing `set_a_cover` path takes them from there. A tiny icon is
not enlarged into a cover: below a threshold it is treated as no cover at all,
because a 22-pixel PNG stretched across a tile looks like a fault.

## Getting the book

The reader taps one button. What it does depends on what the catalog offered,
and the button says which:

| Offered | Button | What happens |
| --- | --- | --- |
| open-access or generic EPUB | **Read** | Fetch, assemble, open |
| plain text and no EPUB | **Read** | The old streaming path, unchanged |
| sample only | **Read sample** | Never dressed up as the whole book |
| buy / borrow / subscribe | **Not available here** with the price when there is one | Never a button that fails |
| nothing readable | no button | Say so on the page |

### The EPUB path

An EPUB is preferred whenever one is offered. See `docs/OPDS.md` for why the
wait is worth it and `crates/kobo-opds/SPEC.md` for the exact ranking.

A zip's central directory is at the end of the file, so an EPUB cannot be read
until its last byte has arrived. Therefore:

1. Fetch in `Range` chunks of `CHUNK_BYTES` into a `ShelfUpload` blob named
   for the publication's identifier. The transport already follows redirects
   and re-sends the range on each hop, which is what Gutenberg's `302` needs.
2. Show real progress — bytes of a known total when the acquisition link
   carried a `length`, and bytes so far when it did not. Never a spinner
   pretending to be a measurement.
3. On the last chunk, `kobo_doc::read` it and hand the `Document` to
   `kobo-read`.
4. Keep the blob. A book already on the shelf opens without the radio, which
   is what `a_book_already_on_the_device_is_read_from_it_rather_than_downloaded`
   already tests for text and must now also hold for EPUBs.
5. A blob that will not parse is thrown away rather than kept forever — the
   same rule the cover cache already follows.

**Check the bytes, not the status.** The first chunk of a supposed EPUB must
begin `PK\x03\x04`, and when it does not, the download stops there and the
screen says the book did not arrive. A `200` proves only that something
arrived: Open Library publishes 49 open-access EPUB links that answer `200`
with an HTML page because they omit a parameter the host requires, and
`kobo_doc::read` will cheerfully turn that page into two blocks of raw markup
and call it a book. Refusing early costs one comparison and saves the reader
from a book made of angle brackets.

Sizes measured from the live catalogs: 1.8 MB (Gutenberg), 769 KB (the 2.0
catalog), 640 KB (Standard Ebooks). `MAX_TASK_BYTES` is 4 MiB and the shelf
holds 256 MiB, so nothing here is near a ceiling.

Verified before this was specified: `kobo_doc::read` parses all three of those
real EPUBs, recovering title, author and 9 to 50 chapters. No third-party
library is needed and the SBOM does not change.

## Details page

The facts shown are the ones the catalog actually stated, and no others. This
rule is inherited and it matters more now, because catalogs vary wildly in what
they carry. No download count unless the catalog gave one; no "Gutenberg ID"
unless the identifier is one; no invented reading time.

`rights` is shown verbatim when a catalog states it, and nothing is shown when
it does not. The old `rights_label` collapsed a Gutendex boolean into one of
three English sentences; catalogs write their own rights statements and this
application is not entitled to rewrite them.

Categories become chips that run a search, as subjects did.

## Storage

- The catalog registry: added catalogs and which one was last open.
- Recent searches, as now, but **per catalog** — a search of Gutenberg is not a
  search of anything else.
- Reading positions, as now, keyed so two catalogs cannot collide over a book
  with the same title.
- Downloaded books, on the shelf.

`MAX_STORE_VALUE` is 256 KiB; keep the registry well inside it and say what the
bound is.

## Tests

Keep every existing test that still describes true behaviour — the page
splitting, the cover retry and caching, the reader controls, the failure
screens, the Back unwinding. Rewrite the ones that assert Gutendex URLs.

### Parity tests

The invariant at the top of this file is worth a test rather than a promise.
Write one catalog twice — the same title, author, description, language,
category and EPUB acquisition link, once as OPDS 1.2 Atom and once as OPDS 2.0
JSON — put each through the application, and assert the screens draw the **same
text**. A difference in wording fails the same way a missing button does.

Fixtures go beside the others as `parity-1.2.xml` and `parity-2.0.json`.

- the shelf drawn from a 1.2 catalog and a 2.0 catalog reads the same
- a book's details drawn from either version read the same
- the search screen offers the same keyboard whichever version supplied the template
- no screen anywhere names the version it is talking to

New behaviour to pin, named as full sentences in the existing style:

- a catalog that answers a navigation feed lists rows rather than an empty shelf
- following a book in a navigation feed opens its details rather than another list
- a feed holding one publication and no navigation is taken as an entry document
- a catalogue cannot send this device to another host (keep, now via `same_origin`)
- an epub is preferred over plain text when a catalog offers both
- a book offered only for sale says so instead of offering a download that fails
- a sample is never presented as the whole book
- an epub arrives in pieces and is not opened until the last one lands
- an epub already on the shelf is read from it rather than downloaded again
- an epub that will not parse is thrown away rather than kept forever
- a data uri thumbnail is decoded rather than fetched
- an icon too small to be a cover is not enlarged into one
- a catalog that answers 401 says access needs a membership
- a recent search belongs to the catalog it was typed into
- covers are filled in only for the shelf page being looked at
- adding a catalog by url keeps it, and a malformed url is refused before it is stored
