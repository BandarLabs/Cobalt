# Send to Kobo

A browser extension that sends a web page, a link or a selection to your
Kobo as a clean article. There are three ways to send the page you are on:

- the Send to Kobo button in the toolbar, with one click;
- Alt+Shift+K, which presses that button;
- Send page to Kobo in the right-click menu, which also has Send link to
  Kobo and Send selection to Kobo.

A notification follows the article as it goes: Sending to your Kobo, then
Landed on your Kobo, or Waiting for your Kobo when the reader is asleep, or
the reason it could not be sent.

A browser may keep a new extension's button in its extensions menu until you
pin it to the toolbar. The shortcut is a suggestion the browser may decline
if the keys are taken; set your own under Keyboard shortcuts on
`chrome://extensions/shortcuts`, or under Manage Extension Shortcuts on
Firefox's `about:addons`.

It does none of the work itself. It reads the page as you see it, with the
hidden parts removed and lazily loaded pictures given their real addresses,
and hands that to Cobalt's `kobo` command on this computer, which makes the
book and sends it over the connection `kobo send` uses. A link is fetched by
`kobo` instead.

## Setting it up

```sh
kobo clip install --browser chrome      # or edge, brave, chromium, firefox
kobo clip install --all                 # every browser found here
```

Install tells the browser where `kobo` is and copies this folder to
`~/.config/kobo/clip/extension`. Then, once:

- Chrome, Edge, Brave and Chromium: open the extensions page, turn on
  Developer mode, choose Load unpacked and pick that folder.
- Firefox: open `about:debugging#/runtime/this-firefox`, choose Load
  Temporary Add-on and pick `manifest.json` in that folder. Firefox forgets
  a temporary add-on when it restarts.

`kobo clip uninstall` takes it all away again. Safari has no extension; see
`docs/companion-cli.md` for the Services menu item install adds instead.

## What it may do

Exactly five permissions, and no sites:

- `contextMenus`, for the three menu items;
- `activeTab` and `scripting`, to read the page you clicked the button on,
  pressed the shortcut on or right-clicked, only then;
- `nativeMessaging`, to hand it to `kobo`;
- `notifications`, to say how it went.

The toolbar button and its shortcut are the manifest's `action` and
`commands` entries, which ask for no permission of their own: a click or the
shortcut is what `activeTab` already waits for.

It has no network access of its own, sends nothing anywhere but to `kobo` on
this computer, and keeps no record of what you sent.

## Fixed identities

The `key` in `manifest.json` fixes the Chromium ID at
`oddgeegniblkiahbnpjbgjoafeanokgi`, and `browser_specific_settings` fixes the
Firefox ID at `send-to-kobo@cobalt.bandarlabs.github.io`. The manifest
`kobo clip install` writes lets in only those, so no other extension can
start `kobo`. Only the public half of the key is here; loading the folder
unpacked needs nothing else. `tools/clip-extension.test.mjs` holds the
manifest, the IDs, the permissions, the shortcut and what a click does to
all of this.
