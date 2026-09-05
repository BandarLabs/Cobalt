# Panels

Panels is a local CBZ reader and companion for your own Komga library. It has
no public-source integrations. CBR is deliberately not
supported: convert it to CBZ, or let your server provide it. PDF comics are
also outside this v1.

CBZ archives use Cobalt's bounded EPUB ZIP parser and pages decode through
`kobo-image`; malformed archives and oversized images are refused. Sideload a
file as `volume.cbz` in the app's directory, then choose **Open sideloaded
comic**. Left-to-right and right-to-left navigation is remembered for every
downloaded volume, along with the last page read.

Komga uses its HTTP Basic credential through `kobo secret set komga`; this app
only names the secret in a runtime task. The app browses nested catalogs,
searches the current catalog page, shows supplied cover art, and downloads
volumes in bounded chunks. Every completed chunk is saved, so an interrupted
download can continue after the app or reader restarts. Completed volumes open
from the offline shelf. Komga is MIT licensed.

```sh
cargo test -p kobo-panels
kobo run --sim --app panels
```

Kavita page streaming and server-side progress reconciliation remain future
work because their server-specific endpoints are not described by a portable
catalog. Local page position is always saved.
