# Cobalt roadmap

Cobalt is an open-source application platform for e-readers. This roadmap
describes the product outcomes the project is working toward. It is a statement
of direction, not a release schedule. Priorities may change as device testing
and community use reveal better answers.

## Product principles

- Keep the default experience small and understandable.
- Make important operations recoverable.
- Work offline wherever practical.
- Keep user-created data portable.
- Give applications only the access they require.
- Build shared platform capabilities when several applications need the same
  behavior.
- Call hardware supported only after device testing, and tell owners
  plainly what is untested on anything else.

## Now

### A shared content foundation

Create a safe way for applications to offer books, articles, papers, documents,
and audio to Cobalt.

The first work establishes stable content identity and an inbox contract before
adding broader library features. It must also define how content already saved
by current applications can be adopted without losing state or forcing a
migration.

Success means:

- An application can offer content without gaining unrestricted file access.
- Duplicate imports can be recognized.
- Malformed or oversized content is rejected before it reaches the library.
- An interrupted import cannot damage accepted content.
- Existing applications continue to work while the shared foundation is
  introduced.

### Reliable daily use

Strengthen recovery across suspend, wake, low storage, interrupted work, and
application failure.

Success means:

- Saved content and reading state survive an unexpected interruption.
- Failed applications cannot prevent an owner from leaving Cobalt.
- Temporary work can be discarded safely after a restart.
- Supported devices pass repeatable suspend and recovery checks.

### Complete the reading experience

The shared reader already keeps bookmarks, highlights and the reading position
across restarts, and looks words up in offline dictionaries. What remains is
finding text inside a book: the reading core can search a document, but the
reader does not offer it yet.

Success means:

- A reader can find text inside a book.
- Reading remains available without a network connection or account.

## Next

### Library

**Books** lists the documents already on the reader. The next step is one place
for saved articles, papers and audio as well, favouring a small set of
dependable views over elaborate organization features.

### Wireless delivery from a phone

`kobo send` and the app commands already send content from a computer over
Wi-Fi. Owners should be able to do the same from a phone.

### Portable reading data

Provide clear backup and export for annotations, reading state, and library
metadata before adding multi-device synchronization.

## Later

- Optional synchronization between devices.
- Vocabulary tools built on the offline dictionaries.
- Private reading insights and statistics.
- Additional document formats.
- More applications built and maintained by the community.
- Support for more devices after hardware testing is complete.

## Shipped

- Installing from the browser, with no tools on the computer.
- Books, with bookmarks, highlights and a kept reading position.
- Offline dictionary lookup. See [docs/DICTIONARIES.md](docs/DICTIONARIES.md).
- Sending files to apps over Wi-Fi with `kobo send`.
- Account connections the owner approves with `kobo connect`, with tokens the
  app never sees.
- Folder sync with a computer through Syncthing.
- Running on Kobos nobody has tested yet, after telling the owner what is
  untested.

## Not currently planned

- Replacing the device operating system or boot process.
- Requiring a cloud account for ordinary reading.
- Unrestricted background applications.
- General-purpose desktop or mobile application compatibility.
- Features that compromise recovery, battery life, or owner control.
- Bundling every available application into the default installation.

## Help shape the roadmap

Cobalt develops in public. Owners and contributors can help by:

- [Requesting an application or product improvement](https://github.com/BandarLabs/Cobalt/issues/26).
- [Reporting a problem](https://github.com/BandarLabs/Cobalt/issues/new).
- [Helping test another device model](CONTRIBUTING.md#device-testing).
- [Building and publishing an application](docs/CONTRIBUTING_APPS.md).
- Improving documentation, translations, tests, or device evidence.

Completed work is recorded in release notes and the repository history. This
file stays focused on work that remains useful to owners.
