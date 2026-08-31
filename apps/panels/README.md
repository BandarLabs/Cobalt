# Panels

Panels is a local CBZ reader and a planned companion for your own Komga or
Kavita server. It has no online-source integrations. CBR is deliberately not
supported: convert it to CBZ, or let your server provide it. PDF comics are
also outside this v1.

CBZ archives use Cobalt's bounded EPUB ZIP parser and pages decode through
`kobo-image`; malformed archives and oversized images are refused. Sideload a
file as `volume.cbz` in the app's directory, then choose **Open sideloaded
CBZ**. Left-to-right and right-to-left navigation is available per session.

Komga uses its HTTP Basic credential through `kobo secret set komga`; this app
only names the secret in a runtime task. The initial MVP tests connectivity but
does not yet decode OPDS catalogs, stream pages, resume downloads, or implement
Kavita. Komga is MIT licensed and Kavita is GPL-3.0 licensed.

```sh
cargo test -p kobo-panels
kobo run --sim --app panels
```
