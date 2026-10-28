# Launcher

The first screen of Cobalt. It opens installed apps and always offers a way
back to the Kobo reader.

## Features

- Four tabs: **Apps**, **Books**, **Settings** and **Kobo reader**.
- Apps are shown as a paged grid. **Previous** and **More apps** appear only
  when there is a page in that direction.
- A recently opened app shows a **Resume** mark.
- **Kobo reader** ends the Cobalt session and returns to the stock reader.

The launcher is an ordinary app built with the public SDK. It has no private
drawing path or hardware access. Its only extra permission is listing and
starting other apps.

## Development

```sh
cargo test -p kobo-launcher
kobo run --sim --app launcher      # in the browser simulator
kobo deploy --device <ip>       # onto a reader over Wi-Fi
```

---

Part of [Cobalt](../../README.md). See [all apps](../../README.md#apps).
