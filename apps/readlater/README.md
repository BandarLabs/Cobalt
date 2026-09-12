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

Configure an HTTPS server in Settings and install its daemon-owned credential:
`kobo secret set wallabag`. The app only names that credential; it never puts a
password, client secret, or token in a request body.

## Dependencies

| Dependency | License | Nature |
| --- | --- | --- |
| [Wallabag](https://wallabag.org/) | MIT | Remote read-later service; not vendored |
| `kobo-sdk`, `kobo-html`, `kobo-json` | Platform | Device UI, storage, rendering and parsing |

`drive.kobo` exercises the setup and offline surface. Wallabag OAuth
password-grant exchange requires kobod's `oauth2-password` credential kind;
this MVP expects that runtime credential and cannot provision it itself.


![Refresh failure retains the current reading list, rendered from an original fixture](../../docs/quality/evidence/readlater-refresh/refresh-failed.png)


![Saving failure retains the previous copy and offers retry](../../docs/quality/evidence/readlater-cache/save-failed.png)
