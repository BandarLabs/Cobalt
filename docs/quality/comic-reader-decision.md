# Shared comic support

Decision, 8 September 2026: keep Panels as the comic library and reader. Put archive inspection, page ordering and bounded page decoding in `kobo-comic`, with reusable `ComicView` reading controls in `kobo-bookview`. Imports and other readers can use this contract without copying Panels internals. A second comic app would duplicate the library, reading position and import workflow.

CBZ is the supported format for this program. CBR is explicitly deferred with the owner's agreement. Identify RAR content and explain that the owner needs a CBZ copy; renaming an extension cannot convert an archive. Do not invoke external RAR tools, bundle an UnRAR binary, or implement a new RAR codec.

## Dependency decision

Use `zip` 4.6.1, pinned, with default features disabled and only DEFLATE through `flate2`'s Rust backend. The published package declares MIT and Rust 1.82, within Cobalt's Rust 1.85.1 floor. Review the resolved dependency graph and ship the applicable notices before release. This is a normal dependency; no upstream implementation or test fixture is copied into Cobalt. Fixtures are generated from original tiny images in our own tests.

Sources: [zip repository](https://github.com/zip-rs/zip2), published Cargo metadata and license files for the pinned package in the Cargo registry, and the resolved Cargo lockfile. The upstream major version is newer; pinning here is for the workspace's MSRV, not a claim that 4.6.1 is latest. Security advisories and ARM build validation remain release checks.

The [unrar wrapper](https://docs.rs/unrar/latest/unrar/#license) offers MIT/Apache terms for the wrapper but separately licenses its embedded C/C++ decoder. A permissive wrapper alone does not meet the requested dependency policy. `unrar-rs` also ports the reference decoder. `compress-tools` adds a native libarchive backend and a separate license/build chain. New pure-Rust RAR implementations have not received enough provenance, compatibility and resource-limit validation in this project to adopt them now. Deferral is preferable to building our own decompressor.

## Resource and input contract

- Inspect the ZIP envelope before constructing the library reader, bounding central-directory bytes and entries. Initially reject ZIP64 and multi-volume archives with conversion guidance.
- Never extract files to disk. Reject absolute paths, traversal, backslashes, links, duplicate names and encrypted entries. Validate all entry names, including non-page metadata.
- Bound archive bytes, total claimed expanded bytes, each page's compressed image bytes and decoded pixels. Only decode the requested page, verifying its CRC through the ZIP reader.
- Use deterministic numeric filename ordering, including nested folders. Ignore metadata and hidden files as pages. Preserve the ordered names so a saved page index is stable for an unchanged archive.
- Share the existing bounded PNG/JPEG decoder. Unsupported formats, corrupt pages and oversized input must produce a specific explanation instead of disappearing from the library.

The shared reader and Panels transfer now use the same 32 MiB archive limit. The archive remains in memory; only page decoding is lazy. This is an admission bound, not a measured memory-headroom guarantee. Larger collections should ultimately read pages from a seekable shelf instead of loading a whole volume into RAM.

Hardware validation will run on the owner's Clara BW after all three PRs are ready. Simulator checks do not establish physical decode latency, memory headroom or display fidelity.


## Reading and metadata

`ComicView` provides page/width fit, bounded zoom and pan, page jump, thumbnail browsing, RTL and landscape spreads with an explicit single-page option. Covers stay separate. The app saves a versioned filename anchor and reading preferences using acknowledged storage; failed saves retain the newest position for an explicit retry.

A bounded subset of ComicInfo supports title, series, number, reading direction and a front-cover index. Optional malformed details produce guidance while leaving pages readable. The [ComicInfo documentation](https://github.com/anansi-project/comicinfo/blob/main/DOCUMENTATION.md) and [version 2 schema reference](https://anansi-project.github.io/docs/comicinfo/schemas/v2.0) are format references only; no upstream schema, implementation or fixture was copied.
