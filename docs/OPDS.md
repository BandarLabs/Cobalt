# OPDS in Cobalt

Gutenbird and Library read book catalogs through OPDS, the Open Publication
Distribution System. This page describes the OPDS client, how it handles real
catalogs, and the rules it follows.

Gutenbird originally used [Gutendex](https://gutendex.com), a JSON service for
Project Gutenberg only. OPDS is the standard that Project Gutenberg, Standard
Ebooks, Calibre servers, Library Simplified and many other libraries publish,
so supporting it lets the same app read any of them.

## Versions

Both versions are in use, so both are supported.

| | OPDS 1.2 | OPDS 2.0 |
| --- | --- | --- |
| Encoding | Atom XML | JSON |
| Media type | `application/atom+xml;profile=opds-catalog` | `application/opds+json` |
| Model | Atom feed of entries | Readium Web Publication Manifest |
| Metadata | Dublin Core | schema.org |
| Status | What most catalogs serve today | The current specification |

Project Gutenberg [plans to retire its XML feeds in 2027](https://www.gutenberg.org/help/mirroring.html)
and is testing a JSON feed.

The version is detected from the response body: a first non-space byte of `{`
means 2.0 and `<` means 1.x. Requests ask for JSON, but many servers ignore the
`Accept` header.

## Catalog notes

These behaviours were found by testing each live service.

### Project Gutenberg

`https://www.gutenberg.org/ebooks/search.opds/?query=…` serves OPDS 1.2 without
authentication. It is the default catalog.

- **Search results are navigation feeds.** Entries are partial, with
  `rel="subsection"` and no download links. Download links are in a separate
  entry document at `/ebooks/{id}.opds`, fetched per book (OPDS 1.2 §5.1.2).
- **Thumbnails are `data:` URIs.** The navigation feed inlines small base64
  PNG icons ([RFC 2397](https://tools.ietf.org/html/rfc2397), allowed by OPDS
  1.2 §5.2.2). Real cover URLs appear only in the entry document. The client
  decodes `data:` URIs instead of fetching them.
- **There is no plain text.** Entry documents offer `epub3`, `epub`, `kf8` and
  `kindle` only. See [Reading](#reading).

### Standard Ebooks

`https://standardebooks.org/feeds/opds` requires a Patrons Circle login: HTTP
Basic authentication with your email address as the username and an empty
password.

The new releases feed at `/feeds/atom/new-releases` is public. It is Atom with
EPUB downloads and `media:thumbnail` covers, but marks downloads
`rel="enclosure"` instead of the OPDS acquisition relation. The client treats
`enclosure` as a download link when nothing better is offered.

### Open Library

The Internet Archive's catalog is at `https://openlibrary.org/opds`. The older
`bookserver.archive.org` address no longer answers.

It is the richest OPDS 2.0 feed tested, with groups, facets, a search template,
linked contributors and subjects, and a cover for every book.

- **Most books can only be borrowed.** On the root feed, 8 of 54 publications
  have an open-access download. The rest are Internet Archive loans that
  declare an authentication document. Gutenbird marks them borrow-only instead
  of offering a download that would fail.
- **Its open-access links are broken.** The Open Access facet lists 49
  downloads, all pointing at Standard Ebooks without the `?source=feed`
  parameter it requires. Without it, the server returns an HTML page with
  status `200`. This is why downloads are checked by content, not status. See
  [Safety](#safety).

### OAPEN

OAPEN serves a feed at `/open-search/discover?query=…&format=opds`, but every
entry links only to an HTML page, with no downloads or covers. It can be added
by hand but is not included by default.

### Test catalogs

- OPDS 2.0: `https://test.opds.io/2.0/home.json`, from
  [opds-community/test-catalog](https://github.com/opds-community/test-catalog).
- OPDS 1.x: [feedbooks/opds-test-catalog](https://github.com/feedbooks/opds-test-catalog).

Both are copied to `crates/kobo-opds/tests/fixtures`, so the conformance tests
run in CI without a network.

## One model for both versions

The screens never show which OPDS version a catalog uses. `kobo-opds` returns
one model, and apps cannot ask which parser produced it. The version is kept
for diagnostics only. Differences are resolved inside the crate:

| The feed has | The app gets |
| --- | --- |
| `dcterms:language` or `metadata.language` | a language |
| `content`, then `summary`, or `metadata.description` | a description |
| `opds:price` or `properties.price` | a price |
| `http://opds-spec.org/acquisition/open-access` or `download` | an open-access download |
| An OpenSearch document or `search{?query}` | a search template |
| `opensearch:totalResults` or `metadata.numberOfItems` | a result count |

OPDS 2.0's short relation names map to the 1.x URIs (OPDS 2.0 §5.3). For
example `download` is `http://opds-spec.org/acquisition/open-access` and
`preview` is `…/sample`.

Parity tests write the same catalog as 1.2 Atom and as 2.0 JSON and check that
both produce the same text on screen.

## Crates

### `kobo-xml`

A pull scanner for elements, attributes and text. It decodes the five XML
entities and numeric references, limits nesting depth, and stops at the first
error while keeping what it has already read, because truncated feeds are
common. Feeds and OPDS 1.2 both use it.

### `kobo-opds`

Parses OPDS bytes into the shared model. It does no I/O, so all its tests run
against local files.

```
Feed
├── title, subtitle, icon, updated
├── links        (self, start, next, previous, first, last, search, crawlable)
├── navigation   (links to other feeds)
├── publications (books)
├── facets       (filtered or reordered views of the same list)
├── groups       (several collections in one 2.0 feed)
└── pagination   (total results, items per page, current page)

Publication
├── title, authors, summary, description, language, issued, rights, categories
├── identifier
├── images       (cover and thumbnail, as a URL or inline data:)
└── acquisition  (generic, open-access, borrow, buy, sample, subscribe)
    └── each with a media type, a price and indirect acquisition
```

## Search

**OPDS 2.0** includes the search template in the feed:

```json
{"rel": "search", "href": "search{?query}", "type": "application/opds+json", "templated": true}
```

**OPDS 1.2** links to an [OpenSearch](https://github.com/dewitt/opensearch)
description document, which must be fetched first:

```xml
<link rel="search" type="application/opensearchdescription+xml"
      href="https://www.gutenberg.org/catalog/osd-books.xml"/>
```

It contains one `Url` per result type:

```xml
<Url type="application/atom+xml"
     template="http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"/>
```

The client picks the `Url` with an OPDS or Atom type, ignoring `text/html` and
suggestion types, and substitutes the percent-encoded query for
`{searchTerms}`. The description is fetched once per catalog and kept.

Gutenberg's template shows two things the client must allow:

- It uses `http://`. The client upgrades it to `https`.
- It is on a different host, `m.gutenberg.org`. Search templates may point to
  another host, because the catalog is declaring where its search lives. Paging
  links may not. See [Safety](#safety).

If a catalog has no OpenSearch document but its `rel="search"` link is already
an OPDS type, that link is used as the template. Otherwise the catalog has no
search, and the screen says so.

### Searching several catalogs

A search can run across several catalogs, with each result labelled by its
source. An app can have [four tasks in flight](../crates/kobo-sdk/src/lib.rs),
and covers use three of them, so catalogs are searched one after another.
Results appear as each catalog answers, and covers load once all searches have
finished. Leaving the screen cancels searches not yet sent.

## Reading

Catalogs mostly offer EPUB, which has to be downloaded completely before it can
be read, because a ZIP file's directory is at its end. The old plain-text path
could show a first page after 256 KB, but lost italics, headings and the table
of contents.

EPUB is preferred whenever it is offered. It is downloaded in `Range` chunks
with progress on screen, parsed by `kobo-doc`, and opened with `kobo-read`.
Plain text is used only when a catalog offers nothing else.

| Format | Used |
| --- | --- |
| `application/epub+zip` | First choice |
| `application/kepub+zip` | Kobo's EPUB variant. It reads, but is not preferred |
| `text/plain` | Only when there is no EPUB |
| `azw3`, `kf8`, `mobi` and others | Never offered, because they cannot be read here |

`kobo-doc::epub` is Cobalt's own EPUB reader. It reads the container, package,
spine and manifest, and tolerates common EPUB faults. No third-party library is
used.

## Request headers

`Task::Fetch` accepts the same validated request headers as `Task::Post`. This
allows content negotiation, which lets one URL serve both OPDS versions, as
Standard Ebooks does. Header names must be HTTP tokens and values visible
ASCII, so an app cannot inject extra headers, including the credential header
it is not allowed to see.

## Safety

Every URL in a feed comes from the server, so the client treats them as
untrusted.

- Only `https` URLs are followed.
- Relative links resolve against the feed's address, per
  [RFC 3986](https://tools.ietf.org/html/rfc3986).
- A `next` link is followed only if it stays on the host the reader chose.
  Otherwise the list simply ends.
- `data:` URIs are decoded, never fetched, and only for image types the device
  can decode.
- Credentials are referred to by name. The runtime attaches them, and the app
  never sees the password.
- **Downloads are checked by content, not by HTTP status.** An EPUB must begin
  with `PK\x03\x04`. Anything else is refused, and the reader is told the book
  did not arrive.
