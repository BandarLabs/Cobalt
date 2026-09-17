# Post

Post is a delivery channel for [Hermes Agent](https://github.com/NousResearch/hermes-agent):
finished correspondence on the reader, rather than a streaming chat screen.
Hermes runs on hardware you control; this app only fetches its completed
letters and posts replies.

Set up the gateway from your computer, then enter its address on the reader:

```sh
kobo post login --gateway <https-address> --token-file <path> --device <reader>
```

The token is pinned to that one gateway, attached by the runtime, and never
enters the app's local state, URLs, request bodies, or logs. Without the
companion CLI, `kobo secret set hermes-post --device <reader>` also works.

## Gateway contract

A compatible gateway serves two routes under its base address:

- `GET /letters?page=<n>&per_page=<m>` answers
  `{"total": N, "items": [{"id", "title", "body"}]}`.
- `POST /replies` takes `{"letter_id", "body", "reply_id"}` and answers
  `{"status": "accepted"}`. The `reply_id` is the app's idempotency key: a
  second post with the same key answers `{"status": "duplicate"}` and must
  not deliver twice.

While the gateway is unavailable, the cached inbox and drafts remain readable
and replies queue locally with their state shown. The VPS bearer-token
transport is implemented; LAN pairing and a scheduled wake/sleep screen are
follow-up work.

![Paginated inbox](../../docs/quality/evidence/post/post-inbox.png)

![A letter with the reply state above it](../../docs/quality/evidence/post/post-reply-sent.png)

![An unsent draft restored after a restart](../../docs/quality/evidence/post/post-draft-restored.png)

Hermes Agent is MIT-licensed by Nous Research. Post is an independent
AGPL-3.0-only Cobalt application and uses Hermes solely as a nominative name.

![Post inbox](screenshots/inbox.png)
