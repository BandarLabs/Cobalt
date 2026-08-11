# OPDS in Cobalt

How Gutenbird stopped being a client of one website and became a client of a
protocol.

## Why this changed

Gutenbird was written against [Gutendex](https://gutendex.com), a JSON front
end to Project Gutenberg's metadata. Gutendex is not a standard: it is one
service, run by one person, answering in a shape nobody else answers in. An
application built on it can read exactly one library.

OPDS — the Open Publication Distribution System — is the shape the rest of the
world answers in. Project Gutenberg publishes one. So do Standard Ebooks,
Feedbooks, every Calibre server, every library running Library Simplified, and
anyone who has ever pointed an ereader at a URL. Speaking OPDS turns Gutenbird
from an application that reads Gutenberg into an application that reads
libraries, of which Gutenberg is one.

There are two versions in the world at once, and both have to be spoken:

| | OPDS 1.2 | OPDS 2.0 |
| --- | --- | --- |
| Encoding | Atom XML | JSON |
| Media type | `application/atom+xml;profile=opds-catalog` | `application/opds+json` |
| Model | Atom feed of entries | Readium Web Publication Manifest |
| Metadata vocabulary | Dublin Core | schema.org |
| Status | What nearly everything serves today | What the specification calls current |

Project Gutenberg [expects to retire its XML feeds in 2027](https://www.gutenberg.org/help/mirroring.html)
and is already testing a JSON one. An application that only reads Atom has a
deadline. An application that only reads JSON cannot read anything yet. So both.

## What the catalogs actually do

Written down because every one of these was discovered by asking the live
service, and each one contradicts something the specification allows you to
assume.

### Project Gutenberg — the default, and the awkward one

`https://www.gutenberg.org/ebooks/search.opds/?query=…` answers OPDS 1.2 over
Atom without authentication. Two things about it shape the whole client:

**A search result is a navigation feed, not an acquisition feed.** Its entries
are *partial* catalog entries carrying `rel="subsection"` and nothing to
download. The acquisition links live in a separate entry document at
`/ebooks/{id}.opds`, which has to be fetched per book. The specification
permits this (§5.1.2, Partial and Complete Catalog Entries); Gutendex did not
work this way, and the shelf has to.

**Its thumbnails are `data:` URIs.** Base64 PNGs inlined into the `href`, as
[RFC 2397](https://tools.ietf.org/html/rfc2397) allows and OPDS 1.2 §5.2.2
explicitly blesses. They are 22×22 category icons in the navigation feed, and
real cover URLs only in the entry document. A client that assumes every image
link is something to fetch over the radio will try to make an HTTP request to
the string `data:image/png;base64,iVBOR…`.

**There is no plain text.** The entry document offers `epub3`, `epub`, `kf8`
and `kindle`, and nothing else. This is the finding that reorganised the
application: Gutenbird's entire reading path streamed `text/plain` in `Range`
chunks so the first page appeared in about a second, and OPDS does not offer
it that text. It offers something better instead. See [Reading](#reading).

### Standard Ebooks — behind a donation

`https://standardebooks.org/feeds/opds` and everything under it answers `401`
with `WWW-Authenticate: Basic realm="Enter your Patrons Circle email address
and leave the password empty."` Feed access is a benefit of donating. The
credential is an email address as the username with an empty password.

Their *new releases* feed at `/feeds/atom/new-releases` is open to everyone and
is usable: Atom, with `application/epub+zip` acquisition links and
`media:thumbnail` covers. It marks its downloads `rel="enclosure"` rather than
`rel="http://opds-spec.org/acquisition"`, which is not what OPDS 1.2 §5.2.1
says, but is what a great many Atom-derived catalogs do. The client treats
`enclosure` as a generic acquisition link of last resort.

### The test catalogs — what conformance is measured against

- **OPDS 2.0**: `https://test.opds.io/2.0/home.json`, from
  [opds-community/test-catalog](https://github.com/opds-community/test-catalog).
  Exercises groups, navigation, publications, images, facets and pagination.
- **OPDS 1.x**: [feedbooks/opds-test-catalog](https://github.com/feedbooks/opds-test-catalog),
  which advertises 50-odd client features: navigation against acquisition
  feeds, pagination, search, full and partial entries, buy and borrow and
  sample acquisition, prices, indirect acquisition, facets and groups.

Both are vendored under `crates/kobo-opds/tests/fixtures` so the conformance
suite runs in CI with no network at all.

### Open Library — the Internet Archive's OPDS, and where it actually lives

`bookserver.archive.org`, the address the Internet Archive's OPDS catalog is
usually given as, resolves to `books-opds0.us.archive.org` and then answers on
neither 443 nor 80 while `archive.org` itself is up. That service is gone.

The Archive's OPDS catalog is now **`https://openlibrary.org/opds`**, and it is
the richest OPDS 2.0 feed of the ones surveyed: `groups` of publications,
`facets` with an active one marked, a templated `search{?query}` link,
contributors and subjects as objects carrying their own links, and a cover for
every book. It is the best real-world exercise of the 2.0 parser there is.

Two things about it are worth knowing before trusting it with a reader's tap:

**Most of its books cannot be downloaded, only borrowed.** Of 54 publications
on the root feed, 8 carry an open-access acquisition link; the rest are
Internet Archive loans, which advertise their requirement honestly through
`properties.authenticate` pointing at an
`application/opds-authentication+json` document. Gutenbird says a book is
borrow-only rather than offering a download that cannot work.

**Its open-access links are broken as published.** Switching to the Open Access
facet gives 49 downloadable publications, and every single one of them points
at Standard Ebooks — without the `?source=feed` parameter Standard Ebooks
requires. The bare URL answers `200` with `Content-Type:
application/xhtml+xml` and an HTML page in the body; the same URL with
`?source=feed` answers `200` with 625 KB of `application/epub+zip`. A client
that trusts the status code hands its reader a book made of markup.

That last one is why [Safety](#safety) requires the bytes to be checked, not
the status.

### OAPEN — not usable

OAPEN answers at `/open-search/discover?query=…&format=opds`, not at `/opds`,
and what it answers is a degenerate feed: all 21 entries carry a single
`rel="alternate"` link to an HTML page, with no acquisition link, no cover, and
no mention of a PDF or an EPUB anywhere in the document. Reaching the books
would mean scraping HTML, which is the thing OPDS exists to avoid.

It remains reachable as a user-added catalog. It is not fit to ship as one.

## One interface, whatever the catalog speaks

The version a catalog speaks is a fact about the wire, not about the reader.
Nothing on the panel may reveal it: no badge saying "OPDS 2.0", no screen that
exists for one version and not the other, no wording that changes because the
metadata arrived as Dublin Core rather than schema.org. A reader who adds a
catalog should be unable to tell which specification it implements, and should
never have to care.

This is enforced rather than intended. `kobo-opds` returns **one** model, and
the application is not given a way to ask which parser produced it — the
version is recorded for diagnostics and is not reachable from any drawing code.
Where the two specifications genuinely differ, the difference is resolved
inside the crate:

| The wire says | The reader sees |
| --- | --- |
| `dcterms:language` / `metadata.language` | a language |
| `content` beating `summary` / `metadata.description` | a description |
| `opds:price` element / `properties.price` object | a price |
| `http://opds-spec.org/acquisition/open-access` / `download` | a book to read |
| OpenSearch description document / `search{?query}` | a keyboard |
| `opensearch:totalResults` / `metadata.numberOfItems` | a count of results |

The one thing a reader may legitimately notice is *speed*: an OPDS 1.2 search
costs a round trip that a 2.0 search does not, and Gutenberg costs a fetch per
cover that an acquisition feed does not. Those are properties of the catalog,
not of the interface, and the screens are the same either way.

Kept honest by **parity tests**: the same logical catalog, written once as
1.2 Atom and once as 2.0 JSON, must produce the same screens. The test asserts
on the text the screen draws, so a difference in wording is a failure the same
way a missing button is.

## The shape of the client

Three new pieces, each of which is useful on its own.

### `kobo-xml`

The Atom scanner that lived inside the Feeds application, promoted to a crate.
It was always general: a pull scanner over elements, attributes and text, with
the five XML entities and numeric references decoded, a depth cap so a
malicious document cannot make it grow, and a policy of stopping at the first
thing it cannot parse rather than guessing — because a feed truncated by a
proxy is far more common than one that is subtly wrong, and half a feed is a
useful answer. OPDS 1.2 is Atom, so it needs exactly this. Feeds keeps
working through it, unchanged.

### `kobo-opds`

The protocol, with no I/O in it at all. It takes bytes and gives back a model;
it never opens a socket, which is what lets the entire conformance suite run
against vendored files. One model serves both versions, because the
application should not have to ask which kind of catalog it is looking at:

```
Feed
├── title, subtitle, icon, updated
├── links        (self, start, next, previous, first, last, search, crawlable)
├── navigation   (somewhere else to go)
├── publications (something to read)
├── facets       (the same list, narrowed or reordered)
├── groups       (2.0's several collections in one feed)
└── pagination   (total results, items per page, current page)

Publication
├── title, authors, summary, description, language, issued, rights, categories
├── identifier
├── images       (cover and thumbnail, either a URL or inline data:)
└── acquisition  (generic, open-access, borrow, buy, sample, subscribe)
    └── each with a media type, a price, and indirect acquisition
```

OPDS 2.0's simplified relation names map onto 1.x's URIs (`download` is
`http://opds-spec.org/acquisition/open-access`, `preview` is `…/sample`, and
so on, per OPDS 2.0 §5.3), so the application sees one vocabulary.

Version is decided by sniffing the response rather than trusting anything: a
body whose first non-space byte is `{` is 2.0 and `<` is 1.x. The `Accept`
header asks for JSON, but a server that ignores it — which is most of them —
gets understood anyway.

### Gutenbird

The same application it was. A shelf of covers, a book, a reader. What changed
is that the shelf is now whatever catalog is pointed at, and there is a screen
for choosing between them.

## Search

Search is part of OPDS, and both versions support it — but they do it so
differently that one costs a round trip the other does not.

**OPDS 2.0** puts the template in the feed:

```json
{"rel": "search", "href": "search{?query}", "type": "application/opds+json", "templated": true}
```

Fill in `query`, fetch, done. Open Library answers such a search with 25
publications and a `numberOfItems` of 3606.

**OPDS 1.2** puts a *link to a document that contains* the template:

```xml
<link rel="search" type="application/opensearchdescription+xml"
      href="https://www.gutenberg.org/catalog/osd-books.xml"/>
```

That document has to be fetched and parsed before a single search can be run,
and inside it is an [OpenSearch](https://github.com/dewitt/opensearch)
description with one `Url` per result type:

```xml
<Url type="application/atom+xml"
     template="http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"/>
```

Pick the `Url` whose type is an OPDS or Atom type — never the `text/html` one,
which is a web page, and never the `application/x-suggestions+json` one, which
is a typeahead. Then substitute `{searchTerms}`, percent-encoded.

Two things about that template are real and both would break a careful client:

- **It is `http://`.** Scheme-upgrade it to `https` rather than refusing it.
  Upgrading is always safe; it is downgrading that is not, and `kobo-net`
  already carries the same relaxation for Gutenberg's download redirects.
- **It is a different host** — `m.gutenberg.org`, not the `www.gutenberg.org`
  the catalog was reached at. So a search template is exempt from the
  same-host rule that governs paging: the catalog is telling the client where
  its own search lives, which is not the same as a redirect walking the client
  somewhere it did not ask to go. Both hosts answer over TLS.

The description document is fetched once per catalog and kept, so the cost is
paid on the first search and never again.

Fallbacks, in order, because a catalog that offers no search at all still has
to do something sensible: a `rel="search"` link that is already an OPDS type
is used directly as a template; failing that, the catalog simply has no search
and the screen says so rather than showing a keyboard that does nothing.

### Searching more than one catalog at once

A reader looking for a book does not care which library has it. So a search may
run against several catalogs, and the results arrive labelled with where they
came from.

What bounds this is not taste but the radio: an application may have
[four tasks in flight](../crates/kobo-sdk/src/lib.rs), three of which the cover
lanes already want, and one search answer is 60 to 120 KB. Firing four searches
at once would spend a quarter of a megabyte before the first row is drawn.

So a federated search is **progressive, not parallel**: catalogs are asked in
turn, each catalog's results are appended as they land, and covers are not
fetched until the searches have settled. The reader sees the first catalog's
results while the second is still being asked. Cancelling — by leaving the
screen — stops the ones not yet sent, which is the whole reason the queue is
not simply flung at the runtime.

## Reading

OPDS hands out EPUBs. This is the part of the change with a cost, and it is
worth being plain about it.

The old path fetched `text/plain` with a byte offset and started drawing after
the first 256 KB, so the first page of a Victorian novel appeared in about a
second on a slow radio. A zip archive cannot do that: the central directory is
at the *end* of the file, so an EPUB is not readable until the last byte of it
has arrived. Half a downloaded book is no book.

**An EPUB is preferred whenever one is offered.** The wait is the price of a
book that has its own italics, headings and table of contents, and that price
is worth paying: the plain text path silently threw all three away, and what it
bought was a first page that arrived sooner than the reader could have started
reading it anyway. So the EPUB is fetched in `Range` chunks into a shelf blob
with the progress on screen, parsed by `kobo-doc` once whole, and handed to
`kobo-read`.

Plain text is a **fallback**, not a fast path: it is chosen only when a catalog
offers no EPUB at all. Some catalogs publish nothing else, and a book that can
only be had as text is still a book.

Ranking, when a catalog offers several editions of the same publication:

| | Why |
| --- | --- |
| `application/epub+zip` | What `kobo-doc` reads, and what nearly every catalog publishes. |
| `application/kepub+zip` | Kobo's own flavour. A valid EPUB with extra spans in it, so it reads, but there is no reason to prefer it. |
| `text/plain` | Only when there is no EPUB. |
| everything else | `azw3`, `kf8` and `mobi` are not read here and are never offered as a download that would fail. |

EPUB support is Cobalt's own — `kobo-doc::epub` reads the container, the
package document, the spine and the manifest, and is deliberately tolerant of
the ways real EPUBs are malformed. No third-party library is added and the
SBOM does not change.

## Sending headers with a fetch

`Task::Fetch` could not carry request headers; only `Task::Post` could. That
made content negotiation impossible, and content negotiation is how a single
URL serves both OPDS versions — which is exactly what Standard Ebooks does and
what Gutenberg's forthcoming JSON feed will do.

`Fetch` now carries the same validated `Vec<Header>` that `Post` has always
carried, with the same rules: names checked against HTTP's token characters,
values against visible ASCII, so that a newline in either cannot let an
application append headers it is not allowed to set — including the credential
header it is never allowed to see.

## Safety

A catalog is a stranger. Every URL in a feed is a value chosen by whoever
answered the request, and following one unchecked is how a redirected catalog
sends the device somewhere it was never pointed.

- Only `https`. The runtime refuses anything else, and a link that would fail
  at download time has already cost the reader a tap.
- Relative references resolve against the feed's own address, per
  [RFC 3986](https://tools.ietf.org/html/rfc3986). Both test catalogs use them
  throughout, so this is not optional.
- A `next` link is followed only when it stays on the host the reader chose.
  Refusing simply ends the shelf, which is what an exhausted catalog looks
  like anyway.
- `data:` URIs are decoded, never fetched, and only when they are an image
  type the device can decode.
- Credentials are named, never held. The application asks the runtime for
  "the Standard Ebooks credential" and the runtime attaches it; the password
  is never in the application's memory, its logs, or its crash dump.
- **A download is checked by its bytes, not by its status.** A `200` proves
  only that something arrived. An EPUB begins `PK\x03\x04`, and anything that
  does not is refused before it reaches the reader, however the server labelled
  it. This is not hypothetical: Open Library publishes 49 open-access EPUB
  links that answer `200` with an HTML page, because they omit a query
  parameter the host requires. A reader who taps Read on one of those should be
  told the book did not arrive, not shown a page of markup.
