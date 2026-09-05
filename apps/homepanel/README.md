# Home Panel

Tiles for a Home Assistant installation, on a panel that costs nothing to
keep showing them. This is a client for Home Assistant; it is not affiliated
with Nabu Casa.

Set the URL once, then install the long-lived access token outside the app:

```sh
kobo secret set homeassistant --device <ip>
```

The URL must be HTTPS. Use Nabu Casa, a reverse proxy with a real
certificate, or install a private CA with `kobo trust set homeassistant
--device <ip>`. Home Panel posts one compact Jinja template per poll and
never puts the token in its URL, body, log, or local store.

Use the `+` action to browse or search Home Assistant devices by their
friendly names. The grid keeps up to twelve tiles, refreshes them every ten
seconds while open, and keeps the last visible state available when the
server cannot be reached. Lights, switches, scenes, scripts, automations, and
buttons can be triggered directly; unsupported domains remain useful as
read-only tiles.

Climate detail controls and a dedicated always-awake wall-panel mode are not
included yet.

![Home Panel grid](screenshots/setup.png)
