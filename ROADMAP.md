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
- Support only hardware that has completed the required device testing.

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

Finish the reading features already under development, with an initial focus on
search, annotations, and durable session behavior.

Success means:

- A reader can find text inside a book.
- Highlights and notes survive restarts.
- Returning to a book restores a stable reading position.
- Reading remains available without a network connection or account.

## Next

### Library

Provide one place to find saved books, articles, papers, documents, and audio.
The first library should favor a small set of dependable views over elaborate
organization features.

### Wireless delivery

Let owners send supported content from a computer or phone to a connected
reader without repeating the initial Cobalt installation process.

### Portable reading data

Provide clear backup and export for annotations, reading state, and library
metadata before adding multi-device synchronization.

### Easier installation

Reduce the tools required for the first USB installation while retaining exact
device checks, visible changes, and a documented recovery path.

## Later

- Optional synchronization between devices.
- Dictionary and vocabulary tools.
- Private reading insights and statistics.
- Additional document formats.
- Owner-approved service connections.
- More applications built and maintained by the community.
- Support for more devices after hardware testing is complete.

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
