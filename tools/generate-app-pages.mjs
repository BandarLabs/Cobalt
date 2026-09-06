import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setupPanel } from "./app-page-setup.mjs";
import { collectRegistry } from "./app-registry.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const catalog = collectRegistry();
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
  backgammon: ["backgammon.png", "Backgammon board on a Kobo showing White's opening roll and 24 touchable points."],
  brief: ["brief.png", "A numbered daily news brief on a Kobo"],
  "calibre-web": ["calibre-web.png", "Private-library list with an Add control and an empty-state explanation."],
  chat: ["chat.png", "An answer displayed for touch-friendly reading on a Kobo"],
  crossword: ["crossword.png", "Crossword grid on a Kobo with a selected cell containing C and touch controls for letters and clues."],
  deck: ["deck.png", "Deck showing six paired-computer controls in a two-column Kobo grid."],
  fanshelf: ["fanshelf.png", "Fanshelf's empty shelf, with Add, Followed tags, and Check updates controls."],
  fieldbook: ["fieldbook.png", "Fieldbook sighting log showing a selected bird, count controls, and a saved lifer notice."],
  flashcards: ["flashcards.png", "Flashcards deck list showing four due cards, today's review count, and a Stats row."],
  frame: ["frame.png", "A full-area monochrome photograph in Frame on a Kobo Clara BW."],
  gallery: ["components.png", "Cobalt typography and interface components on a Kobo"],
  grimoire: ["grimoire.png", "Grimoire initiative order showing the active combatant and round counter on a Kobo."],
  gutenbird: ["gutenbird.png", "A shelf of books from an OPDS library on a Kobo"],
  habits: ["habits.png", "Habits today screen on a Kobo Clara BW, with daily and weekday streak tasks."],
  hn: ["hackernews.png", "A ranked list of Hacker News stories on a Kobo"],
  homepanel: ["homepanel.png", "Home Panel connection screen with HTTPS Home Assistant URL entry and named-secret instructions."],
  inkling: ["inkling.png", "A solved Inkling five-letter daily puzzle with grayscale shape feedback."],
  kitchencard: ["kitchencard.png", "Kitchen Card showing a large cooking instruction with Steps and Ingredients tabs."],
  lichess: ["lichess.png", "Lichess on Kobo showing Puzzle and Play tiles, a daily puzzle row, and puzzle difficulty."],
  logicpack: ["logicpack.png", "Logic Pack's Minesweeper board after a revealed cell and contradiction check."],
  launcher: ["launcher.png", "The Cobalt launcher showing installed apps on a Kobo"],
  lichess: ["lichess.png", "Responsive Lichess time-control tiles and puzzles on a Kobo"],
  magnet: ["magnet.png", "The Kobo hall sensor responding to a magnet"],
  morse: ["morse.png", "A letter filling the Kobo screen while the front light sends Morse code"],
  musicstand: ["musicstand.png", "Music Stand showing a marked half-page score view for Bach's Cello Suite No. 1."],
  needles: ["needles.png", "Needles pattern screen with row and repeat counters and a large +1 row button."],
  nonograms: ["nonograms.png", "Nonogram puzzle on a Kobo showing numbered cells, row clues, and guided marking mode."],
  panels: ["panels.png", "Panels library with controls to open an added comic or browse Komga."],
  paperterm: ["paperterm.png", "Paperterm pairing screen with computer address field and on-screen keyboard on a Kobo Clara BW."],
  parlor: ["parlor.png", "Reversi opening board showing four legal moves and touch controls."],
  parser: ["parser.png", "Parser's book-like transcript after taking a brass lamp and entering the garden."],
  post: ["post.png", "Post inbox showing completed Hermes letters, newest first."],
  pubquiz: ["pubquiz.png", "Pub Quiz pass-around question with four large answer choices for Ada."],
  readlater: ["readlater.png", "Read Later setup screen showing Wallabag credential instructions."],
  rss: ["feeds.png", "Subscribed feeds and articles in the Feeds app on a Kobo"],
  "rss-miniflux": ["rss-miniflux.png", "RSS Reader starter directory listing Science News, engineering blogs, and long-form writing."],
  settings: ["settings.png", "Battery status and hardware information in Cobalt Settings"],
  sidekick: ["sidekick.png", "Sidekick multi-agent board showing distinct coding-agent sessions and pending approvals."],
  store: ["store.png", "The Cobalt App Store listing installed and available apps"],
  sudoku: ["sudoku.png", "A Sudoku game designed for the Kobo touch screen"],
  syncthing: ["syncthing.png", "Sync folders showing receive-only vault, frame, books, and send-only out."],
  terminal: ["terminal.png", "A shell and touch keyboard on a Kobo"],
  tictactoe: ["tictactoe.png", "A completed game of tic-tac-toe on a Kobo"],
  todo: ["todo.png", "A to-do list with completed items on a Kobo"],
  vault: ["vault.png", "Vault first-run screen on a Kobo Clara BW, showing the commands to push a note vault."],
  verses: ["verses.png", "Verses displaying a public-domain daily poem in a spacious Kobo reading layout."],
  "zotero-reader": ["zotero-reader.png", "Reading a paper with structured layout and Zotero metadata on a Kobo"]
};
const screenshotFor = app => {
  const screenshot = screenshots[app.id];
  return screenshot || [
    "store.png",
    `${app.display_name} available from the signed Cobalt App Store`
  ];
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
const jsonLd = value => JSON.stringify(value, null, 2).replaceAll("<", "\\u003c");
const scriptHash = value => createHash("sha256").update(value).digest("base64");
const pageDescription = app => {
  if (app.page_description === undefined) return app.summary;
  if (
    typeof app.page_description !== "string"
    || app.page_description.trim().length === 0
    || app.page_description.length > 512
  ) {
    throw new Error(`${app.id} page_description must be a non-empty string of at most 512 characters`);
  }
  return app.page_description;
};
const installableDescription = app =>
  `${pageDescription(app)} Install it on a supported Kobo e-reader with Cobalt.`;
const systemDescription = app =>
  `${app.summary} Included with Cobalt on supported Kobo e-readers.`;
const appSchema = (app, canonical, screenshot, screenshotAlt) => ({
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "SoftwareApplication",
      name: app.display_name,
      description: pageDescription(app),
      url: canonical,
      image: {
        "@type": "ImageObject",
        url: `https://bandarlabs.github.io/Cobalt/media/site/apps/${screenshot}`,
        width: 1072,
        height: 1448,
        caption: screenshotAlt
      },
      applicationSuite: "Cobalt",
      applicationCategory: "SoftwareApplication",
      operatingSystem: "Cobalt on supported Kobo e-readers",
      ...(app.version ? { softwareVersion: app.version } : {}),
      isAccessibleForFree: true,
      offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
      installUrl: canonical
    },
    {
      "@type": "BreadcrumbList",
      itemListElement: [
        {
          "@type": "ListItem",
          position: 1,
          name: "Cobalt",
          item: "https://bandarlabs.github.io/Cobalt/"
        },
        {
          "@type": "ListItem",
          position: 2,
          name: "Apps",
          item: "https://bandarlabs.github.io/Cobalt/#apps"
        },
        {
          "@type": "ListItem",
          position: 3,
          name: app.display_name,
          item: canonical
        }
      ]
    }
  ]
});

for (const app of catalog.apps) {
  const id = escape(app.id);
  const name = escape(app.display_name);
  const summary = escape(pageDescription(app));
  const description = escape(installableDescription(app));
  const capabilities = app.capabilities.length
    ? app.capabilities.map(escape).join(", ")
    : "No additional permissions";
  const canonical = `https://bandarlabs.github.io/Cobalt/apps/${id}/`;
  const [screenshot, screenshotAlt] = screenshotFor(app);
  const image = `https://bandarlabs.github.io/Cobalt/media/site/apps/${screenshot}`;
  const structuredData = jsonLd(appSchema(app, canonical, screenshot, screenshotAlt));
  const structuredDataHash = scriptHash(structuredData);
  const prerequisites = setupPanel(app);
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Install ${name} on Kobo | Cobalt</title>
<meta name="description" content="${description}">
<link rel="canonical" href="${canonical}">
<link rel="icon" href="../../logo.svg" type="image/svg+xml">
<meta property="og:type" content="website">
<meta property="og:title" content="Install ${name} on Kobo | Cobalt">
<meta property="og:description" content="${description}">
<meta property="og:url" content="${canonical}">
<meta property="og:site_name" content="Cobalt">
<meta property="og:image" content="${image}">
<meta property="og:image:width" content="1072">
<meta property="og:image:height" content="1448">
<meta property="og:image:alt" content="${escape(screenshotAlt)}">
<meta name="twitter:card" content="summary">
<meta name="twitter:title" content="Install ${name} on Kobo | Cobalt">
<meta name="twitter:description" content="${description}">
<meta name="twitter:image" content="${image}">
<meta name="twitter:image:alt" content="${escape(screenshotAlt)}">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self' 'sha256-${structuredDataHash}'; style-src 'self'; img-src 'self'; connect-src https://cobalt-install-relay.anandabhishek.workers.dev; base-uri 'none'; form-action 'self'">
<script type="application/ld+json">${structuredData}</script>
<link rel="stylesheet" href="../install.css">
</head>
<body>
<header class="masthead">
  <div class="wrap">
    <a class="brand" href="../../"><img src="../../logo.svg" alt="Cobalt" width="81" height="34"></a>
    <nav class="top" aria-label="Main navigation">
      <a class="active" href="../../#apps">Apps</a>
      <a href="../../developers.html">Developers</a>
      <a href="../../faq.html">FAQ</a>
      <a href="../../#store">Store</a>
      <a href="../../#install">Install</a>
      <a href="../../#contributing">Contributing</a>
      <a href="https://github.com/BandarLabs/Cobalt">GitHub</a>
    </nav>
  </div>
</header>
<main class="wrap" data-app-id="${id}" data-minimum-cobalt-version="${escape(app.minimum_cobalt_version)}">
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
  </div>${prerequisites}
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
  <aside class="community">
    <strong>Want another Kobo app?</strong>
    Request it, suggest a feature or report a bug in
    <a href="https://www.reddit.com/r/CobaltForKobo/">r/CobaltForKobo</a>.
  </aside>
</main>
<footer>
  <div class="wrap">
    <span>Cobalt · AGPL-3.0</span>
    <nav aria-label="Footer navigation">
      <a href="../../">Home</a>
      <a href="https://www.reddit.com/r/CobaltForKobo/">Community</a>
      <a href="https://github.com/BandarLabs/Cobalt">GitHub</a>
    </nav>
  </div>
</footer>
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
  const description = escape(systemDescription(app));
  const canonical = `https://bandarlabs.github.io/Cobalt/apps/${id}/`;
  const [screenshot, screenshotAlt] = screenshotFor(app);
  const image = `https://bandarlabs.github.io/Cobalt/media/site/apps/${screenshot}`;
  const structuredData = jsonLd(appSchema(app, canonical, screenshot, screenshotAlt));
  const structuredDataHash = scriptHash(structuredData);
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${name} for Kobo | Cobalt</title>
<meta name="description" content="${description}">
<link rel="canonical" href="${canonical}">
<link rel="icon" href="../../logo.svg" type="image/svg+xml">
<meta property="og:type" content="website">
<meta property="og:title" content="${name} for Kobo | Cobalt">
<meta property="og:description" content="${description}">
<meta property="og:url" content="${canonical}">
<meta property="og:site_name" content="Cobalt">
<meta property="og:image" content="${image}">
<meta property="og:image:width" content="1072">
<meta property="og:image:height" content="1448">
<meta property="og:image:alt" content="${escape(screenshotAlt)}">
<meta name="twitter:card" content="summary">
<meta name="twitter:title" content="${name} for Kobo | Cobalt">
<meta name="twitter:description" content="${description}">
<meta name="twitter:image" content="${image}">
<meta name="twitter:image:alt" content="${escape(screenshotAlt)}">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'sha256-${structuredDataHash}'; style-src 'self'; img-src 'self'; base-uri 'none'">
<script type="application/ld+json">${structuredData}</script>
<link rel="stylesheet" href="../install.css">
</head>
<body>
<header class="masthead">
  <div class="wrap">
    <a class="brand" href="../../"><img src="../../logo.svg" alt="Cobalt" width="81" height="34"></a>
    <nav class="top" aria-label="Main navigation">
      <a class="active" href="../../#apps">Apps</a>
      <a href="../../developers.html">Developers</a>
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
  <aside class="community">
    <strong>Want another Kobo app?</strong>
    Request it, suggest a feature or report a bug in
    <a href="https://www.reddit.com/r/CobaltForKobo/">r/CobaltForKobo</a>.
  </aside>
</main>
<footer>
  <div class="wrap">
    <span>Cobalt · AGPL-3.0</span>
    <nav aria-label="Footer navigation">
      <a href="../../">Home</a>
      <a href="https://www.reddit.com/r/CobaltForKobo/">Community</a>
      <a href="https://github.com/BandarLabs/Cobalt">GitHub</a>
    </nav>
  </div>
</footer>
</body>
</html>
`;
  const output = resolve(root, "docs/apps", app.id, "index.html");
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, html);
}

const sitemapUrls = [
  "https://bandarlabs.github.io/Cobalt/",
  "https://bandarlabs.github.io/Cobalt/developers.html",
  "https://bandarlabs.github.io/Cobalt/faq.html",
  "https://bandarlabs.github.io/Cobalt/sdk.html",
  ...[...catalog.apps, ...systemApps].map(
    app => `https://bandarlabs.github.io/Cobalt/apps/${app.id}/`
  )
];
const sitemap = `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${sitemapUrls.map(url => `  <url><loc>${url}</loc></url>`).join("\n")}
</urlset>
`;
writeFileSync(resolve(root, "docs/sitemap.xml"), sitemap);
