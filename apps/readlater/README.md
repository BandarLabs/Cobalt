# Read Later

A Wallabag reading app for the Kobo. Save links from Wallabag's phone or
browser tools, then sync their extracted articles to the reader.

Fetched article bodies survive metadata refreshes during the current session.
Unreadable list responses leave current articles unchanged, and replies from a
previous server or credential cannot replace the current list. The queue's
**Sync** control retries a failed refresh.

Articles now use acknowledged local snapshots scoped to the server and credential
name. Saves keep a previous copy until the new content and its pointer are
acknowledged. A failed save shows **Retry saving**; Settings remains available
from the reading list. The collection is bounded to 8 MiB, and extracted article
text is stored verbatim, including Unicode and paragraph breaks.

Full simulator restart/recovery validation, acknowledged archive/star replay,
filtering and reading pagination remain under LATER-01 through LATER-06. The app
is not yet a complete offline Wallabag client.

![Read Later setup on the Clara BW simulator](screenshots/readlater-setup.png)

Sign in from a computer with `kobo readlater login`: it completes Wallabag's
OAuth exchange and delivers the session to the reader, and the app renews the
token itself when it expires. The token is installed as a server-bound account,
so it can only ever be sent to your own Wallabag host. The app only ever names
the `wallabag` credential; it never puts a password, client secret, or token in
a request body. Without the companion CLI, a bearer token installed with
`kobo secret set wallabag` and an HTTPS server in Settings also work.

## Dependencies

| Dependency | License | Nature |
| --- | --- | --- |
| [Wallabag](https://wallabag.org/) | MIT | Remote read-later service; not vendored |
| `kobo-sdk`, `kobo-html`, `kobo-json` | Platform | Device UI, storage, rendering and parsing |

`drive.kobo` exercises the setup and offline surface. Archives and stars are
written to an acknowledged outbox and replayed to Wallabag on the next sync;
the reading list is split into Unread, Starred and Archive tabs.


![Refresh failure retains the current reading list, rendered from an original fixture](../../docs/quality/evidence/readlater-refresh/refresh-failed.png)


![Saving failure retains the previous copy and offers retry](../../docs/quality/evidence/readlater-cache/save-failed.png)
