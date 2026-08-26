import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const catalog = JSON.parse(readFileSync(resolve(root, "apps/catalog.json"), "utf8"));
const systemApps = [
  {
    id: "launcher",
    display_name: "Launcher",
    summary: "Opens installed apps and always keeps a route back to the Kobo reader."
  },
  {
    id: "store",
    display_name: "App Store",
    summary: "Installs, updates, removes and reinstalls signed apps over Wi-Fi."
  },
  {
    id: "terminal",
    display_name: "Terminal",
    summary: "A panel-native shell with keys that send input immediately."
  },
  {
    id: "settings",
    display_name: "Settings",
    summary: "Connectivity, hardware and platform updates, kept separate from Store."
  }
];
const screenshots = {
  arxiv: ["arxiv.png", "The newest machine learning preprints listed in the arXiv app on a Kobo"],
  audiobook: ["audiobook.png", "An audiobook player with cover art and playback controls on a Kobo"],
  brief: ["brief.png", "A numbered daily news brief on a Kobo"],
  chat: ["chat.png", "An answer displayed for touch-friendly reading on a Kobo"],
  gallery: ["components.png", "Cobalt typography and interface components on a Kobo"],
  gutenbird: ["gutenbird.png", "A shelf of books from an OPDS library on a Kobo"],
  hn: ["hackernews.png", "A ranked list of Hacker News stories on a Kobo"],
  launcher: ["launcher.png", "The Cobalt launcher showing installed apps on a Kobo"],
  magnet: ["magnet.png", "The Kobo hall sensor responding to a magnet"],
  morse: ["morse.png", "A letter filling the Kobo screen while the front light sends Morse code"],
  rss: ["feeds.png", "Subscribed feeds and articles in the Feeds app on a Kobo"],
  settings: ["settings.png", "Battery status and hardware information in Cobalt Settings"],
  sidekick: ["sidekick.png", "A coding-agent request with tappable responses on a Kobo"],
  store: ["store.png", "The Cobalt App Store listing installed and available apps"],
  sudoku: ["sudoku.png", "A Sudoku game designed for the Kobo touch screen"],
  terminal: ["terminal.png", "A shell and touch keyboard on a Kobo"],
  tictactoe: ["tictactoe.png", "A completed game of tic-tac-toe on a Kobo"],
  todo: ["todo.png", "A to-do list with completed items on a Kobo"]
};
const appsRoot = resolve(root, "docs/apps");
for (const app of catalog.apps) {
  if (
    typeof app.id !== "string"
    || app.id.length === 0
    || app.id.length > 32
    || !/^[a-z][a-z0-9-]*$/.test(app.id)
    || app.id.endsWith("-")
    || app.id.includes("--")
  ) {
    throw new Error(`invalid app id: ${String(app.id)}`);
  }
}
const appIds = new Set([...catalog.apps, ...systemApps].map(app => app.id));

mkdirSync(appsRoot, { recursive: true });
for (const entry of readdirSync(appsRoot, { withFileTypes: true })) {
  if (entry.isDirectory() && !appIds.has(entry.name)) {
    rmSync(resolve(appsRoot, entry.name), { recursive: true });
  }
}

const escape = value => value
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;");

for (const app of catalog.apps) {
  const id = escape(app.id);
  const name = escape(app.display_name);
  const summary = escape(app.summary);
  const capabilities = app.capabilities.length
    ? app.capabilities.map(escape).join(", ")
    : "No additional permissions";
  const canonical = `https://bandarlabs.github.io/Cobalt/apps/${id}/`;
  const [screenshot, screenshotAlt] = screenshots[app.id];
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${name} for Cobalt</title>
<meta name="description" content="${summary}">
<link rel="canonical" href="${canonical}">
<meta property="og:type" content="website">
<meta property="og:title" content="${name} for Cobalt">
<meta property="og:description" content="${summary}">
<meta property="og:url" content="${canonical}">
<meta property="og:image" content="https://bandarlabs.github.io/Cobalt/media/site/og-card.jpg">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="${name} for Cobalt">
<meta name="twitter:description" content="${summary}">
<meta name="twitter:image" content="https://bandarlabs.github.io/Cobalt/media/site/og-card.jpg">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src https://cobalt-install-relay.anandabhishek.workers.dev; base-uri 'none'; form-action 'self'">
<link rel="stylesheet" href="../install.css">
</head>
<body>
<header class="masthead">
  <div class="wrap">
    <a class="brand" href="../../"><img src="../../logo.svg" alt="Cobalt" width="81" height="34"></a>
    <nav class="top" aria-label="Main navigation">
      <a class="active" href="../../#apps">Apps</a>
      <a href="../../sdk.html">SDK</a>
      <a href="../../faq.html">FAQ</a>
      <a href="../../#store">Store</a>
      <a href="../../#install">Install</a>
      <a href="../../#contributing">Contributing</a>
      <a href="https://github.com/BandarLabs/Cobalt">GitHub</a>
    </nav>
  </div>
</header>
<main class="wrap" data-app-id="${id}">
  <div class="app-hero">
    <div class="app-copy">
      <p class="eyebrow">Kobo app</p>
      <h1>${name}</h1>
      <p class="summary">${summary}</p>
      <div class="meta"><span>Version ${escape(app.version)}</span><span>${capabilities}</span></div>
    </div>
    <figure class="app-shot">
      <img src="../../media/site/apps/${screenshot}" width="1072" height="1448" alt="${escape(screenshotAlt)}">
    </figure>
  </div>
  <section class="panel" id="pair-panel">
    <p class="eyebrow">Install with Cobalt</p>
    <h2>Link your Kobo to install</h2>
    <p>On your Kobo, open <strong>App Store</strong>, then <strong>Install links</strong>. Scan the QR code, or enter the pairing code and verification key shown there.</p>
    <form id="pair-form">
      <div class="field">
        <label for="pair-code">Pairing code</label>
        <input id="pair-code" name="code" inputmode="text" autocomplete="one-time-code" maxlength="8" required>
      </div>
      <div class="field" id="pair-secret-field">
        <label for="pair-secret">Verification key</label>
        <input class="secret" id="pair-secret" name="secret" inputmode="text" autocomplete="off" autocapitalize="none" spellcheck="false" maxlength="45" required>
      </div>
      <button type="submit">Link Kobo</button>
    </form>
    <p class="status" id="pair-status" role="status" aria-live="polite"></p>
  </section>
  <section class="panel setup" id="setup-panel">
    <p class="eyebrow">Cobalt not installed?</p>
    <h2>Set up Cobalt first</h2>
    <p>Install Cobalt once over USB, then return to this page. Future apps install and update over Wi-Fi without reconnecting the cable.</p>
    <ol>
      <li>Check that your Kobo model and firmware are supported.</li>
      <li>Connect the charged Kobo to a Mac or Linux computer and follow the setup guide.</li>
      <li>Restart your Kobo, open Cobalt App Store, and return here to link it.</li>
    </ol>
    <a class="button-link secondary" href="https://github.com/BandarLabs/Cobalt/blob/main/docs/INSTALL.md">Set up Cobalt</a>
  </section>
  <section class="panel" id="install-panel" hidden>
    <h2>Install on <span id="device-name">your Kobo</span></h2>
    <p>Cobalt verifies the signed catalog and app package before changing the installed copy.</p>
    <div class="actions">
      <button type="button" id="install">Install ${name}</button>
      <button type="button" id="forget" class="secondary">Forget this Kobo</button>
    </div>
    <p class="status" id="install-status" role="status" aria-live="polite"></p>
  </section>
</main>
<script src="../install.js" defer></script>
</body>
</html>
`;
  const output = resolve(root, "docs/apps", app.id, "index.html");
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, html);
}

for (const app of systemApps) {
  const id = escape(app.id);
  const name = escape(app.display_name);
  const summary = escape(app.summary);
  const canonical = `https://bandarlabs.github.io/Cobalt/apps/${id}/`;
  const [screenshot, screenshotAlt] = screenshots[app.id];
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${name} for Cobalt</title>
<meta name="description" content="${summary}">
<link rel="canonical" href="${canonical}">
<meta property="og:type" content="website">
<meta property="og:title" content="${name} for Cobalt">
<meta property="og:description" content="${summary}">
<meta property="og:url" content="${canonical}">
<meta property="og:image" content="https://bandarlabs.github.io/Cobalt/media/site/og-card.jpg">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="${name} for Cobalt">
<meta name="twitter:description" content="${summary}">
<meta name="twitter:image" content="https://bandarlabs.github.io/Cobalt/media/site/og-card.jpg">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'">
<link rel="stylesheet" href="../install.css">
</head>
<body>
<header class="masthead">
  <div class="wrap">
    <a class="brand" href="../../"><img src="../../logo.svg" alt="Cobalt" width="81" height="34"></a>
    <nav class="top" aria-label="Main navigation">
      <a class="active" href="../../#apps">Apps</a>
      <a href="../../sdk.html">SDK</a>
      <a href="../../faq.html">FAQ</a>
      <a href="../../#store">Store</a>
      <a href="../../#install">Install</a>
      <a href="../../#contributing">Contributing</a>
      <a href="https://github.com/BandarLabs/Cobalt">GitHub</a>
    </nav>
  </div>
</header>
<main class="wrap">
  <div class="app-hero">
    <div class="app-copy">
      <p class="eyebrow">Cobalt system app</p>
      <h1>${name}</h1>
      <p class="summary">${summary}</p>
      <div class="meta"><span>Included with Cobalt</span></div>
    </div>
    <figure class="app-shot">
      <img src="../../media/site/apps/${screenshot}" width="1072" height="1448" alt="${escape(screenshotAlt)}">
    </figure>
  </div>
  <section class="panel setup">
    <p class="eyebrow">No separate install needed</p>
    <h2>Available after Cobalt setup</h2>
    <p>${name} is part of the Cobalt platform and is installed automatically with Cobalt. It does not need a separate App Store download.</p>
    <a class="button-link" href="../../#install">Set up Cobalt</a>
  </section>
</main>
</body>
</html>
`;
  const output = resolve(root, "docs/apps", app.id, "index.html");
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, html);
}

const home = readFileSync(resolve(root, "docs/index.html"), "utf8");
for (const app of [...catalog.apps, ...systemApps]) {
  if (!home.includes(`href="apps/${app.id}/"`)) {
    throw new Error(`docs/index.html does not link to app page: ${app.id}`);
  }
}
