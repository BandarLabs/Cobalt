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
  arxiv: ["arxiv.png", "The newest Artificial Intelligence preprints listed newest first in the Preprints app on a Kobo"],
  audiobook: ["audiobook.png", "An audiobook player with cover art and playback controls on a Kobo"],
  backgammon: ["backgammon.png", "Backgammon board on a Kobo after Black opened with 4 and 6, with dice, cube and match score."],
  birds: ["birds.png", "A labelled collage of public-domain bird plates filling a Kobo screen."],
  brief: ["brief.png", "A numbered daily news brief on a Kobo"],
  "calibre-web": ["calibre-web.png", "A book from a calibre-web library open on a Kobo, with text-size and front-light controls and the page count."],
  chat: ["chat.png", "An answer displayed for touch-friendly reading on a Kobo"],
  crossword: ["crossword.png", "Crossword grid on a Kobo with the first answer filled in and numbered cells."],
  deck: ["deck.png", "Deck paired with a computer, showing Test, Format and Deploy command pads."],
  fanshelf: ["fanshelf.png", "A followed work in Fanshelf naming its author, fandom, rating and chapter count, with Read and Check updates controls."],
  fieldbook: ["fieldbook.png", "Fieldbook outing screen tallying an American Robin from a pushed field pack."],
  flashcards: ["flashcards.png", "A Flashcards review showing the revealed answer with Again, Hard, Good and Easy rating buttons."],
  frame: ["frame.png", "A full-area monochrome photograph in Frame on a Kobo Clara BW."],
  gallery: ["components.png", "Cobalt typography and interface components on a Kobo"],
  grimoire: ["grimoire.png", "Grimoire initiative order showing the active combatant and round counter on a Kobo."],
  gutenbird: ["gutenbird.png", "A shelf of books from an OPDS library on a Kobo"],
  habits: ["habits.png", "Habits today screen on a Kobo Clara BW, with daily and weekday streak tasks."],
  hn: ["hackernews.png", "A ranked list of Hacker News stories on a Kobo"],
  homepanel: ["homepanel.png", "Home Panel tile grid on a Kobo showing four Home Assistant tiles with the last refresh time."],
  inkling: ["inkling.png", "A solved Inkling five-letter daily puzzle with grayscale shape feedback."],
  kitchencard: ["kitchencard.png", "Kitchen Card showing a large cooking instruction with Steps and Ingredients tabs."],
  lichess: ["lichess.png", "Lichess on Kobo with Account/Games and Puzzles tiles plus rapid and classical time controls."],
  logicpack: ["logicpack.png", "Logic Pack's Minesweeper board after a revealed cell and contradiction check."],
  launcher: ["launcher.png", "The Cobalt launcher showing installed apps on a Kobo"],
  magnet: ["magnet.png", "The Kobo hall sensor responding to a magnet"],
  morse: ["morse.png", "A letter filling the Kobo screen while the front light sends Morse code"],
  musicstand: ["musicstand.png", "Music Stand showing the Prelude from Bach's Cello Suite No. 1 as a full-page score."],
  needles: ["needles.png", "Needles pattern screen with row and repeat counters and a large +1 row button."],
  nonograms: ["nonograms.png", "Actual Nonograms simulator capture showing a selected square and its matching row and column clues."],
  panels: ["panels.png", "Panels library in the Clara BW simulator showing the original A small garden cover and saved page 2 of 4."],
  paperterm: ["paperterm.png", "Paperterm sharing a real laptop terminal in portrait, with its keyboard open on a Clara BW simulator."],
  parlor: ["parlor.png", "Reversi opening board showing four legal moves and touch controls."],
  parser: ["parser.png", "Parser's book-like transcript after taking a brass lamp and entering the garden."],
  post: ["post.png", "Post inbox showing completed Hermes letters, newest first."],
  pubquiz: ["pubquiz.png", "Pub Quiz pass-around question with four large answer choices for Ada."],
  readlater: ["readlater.png", "Read Later setup screen showing Wallabag credential instructions."],
  rss: ["feeds.png", "Subscribed feeds and articles in the Feeds app on a Kobo"],
  "rss-miniflux": ["rss-miniflux.png", "A Miniflux article open on a Kobo with text-size and front-light controls."],
  settings: ["settings.png", "Battery status and hardware information in Cobalt Settings"],
  sidekick: ["sidekick.png", "Sidekick multi-agent board showing distinct coding-agent sessions and pending approvals."],
  store: ["store.png", "The Cobalt App Store listing installed and available apps"],
  sudoku: ["sudoku.png", "An original Sudoku puzzle with pencil notes, selected keys and a highlighted row and column"],
  syncthing: ["syncthing.png", "Sync folders showing receive-only vault, frame, books, and send-only out."],
  terminal: ["terminal.png", "A shell and touch keyboard on a Kobo"],
  tictactoe: ["tictactoe.png", "A completed game of tic-tac-toe on a Kobo"],
  todo: ["todo.png", "A to-do list with completed items on a Kobo"],
  vault: ["vault.png", "Vault home on a Kobo with four synced notes and Browse, Tags, Recent and Search rows."],
  verses: ["verses.png", "Verses displaying a public-domain daily poem in a spacious Kobo reading layout."],
  "zotero-reader": ["zotero-reader.png", "Reading a paper with structured layout and Zotero metadata on a Kobo"]
};
// Categories are a property of the listing, not of the app, so they live here
// rather than in cobalt-app.json. A manifest field would ship inside every
// package and force all 45 apps to publish a new version for a line of text
// that only the website draws.
const categories = {
  arxiv: "Reading",
  audiobook: "Audio",
  backgammon: "Games",
  birds: "Devices",
  brief: "Reading",
  "calibre-web": "Reading",
  chat: "Developer",
  crossword: "Games",
  deck: "Devices",
  fanshelf: "Reading",
  fieldbook: "Productivity",
  flashcards: "Productivity",
  frame: "Devices",
  gallery: "Developer",
  grimoire: "Reference",
  gutenbird: "Reading",
  habits: "Productivity",
  hn: "Reading",
  homepanel: "Devices",
  inkling: "Games",
  kitchencard: "Productivity",
  lichess: "Games",
  logicpack: "Games",
  magnet: "Developer",
  morse: "Devices",
  musicstand: "Reference",
  needles: "Productivity",
  nonograms: "Games",
  panels: "Reading",
  paperterm: "Devices",
  parlor: "Games",
  parser: "Games",
  post: "Productivity",
  pubquiz: "Games",
  readlater: "Reading",
  rss: "Reading",
  "rss-miniflux": "Reading",
  sidekick: "Developer",
  sudoku: "Games",
  syncthing: "Devices",
  tictactoe: "Games",
  todo: "Productivity",
  vault: "Reference",
  verses: "Reading",
  "zotero-reader": "Reading"
};
const categoryFor = app => {
  const category = categories[app.id];
  if (!category) {
    throw new Error(`${app.id} has no listing category; add one to categories`);
  }
  return category;
};
const screenshotFor = app => {
  const screenshot = screenshots[app.id];
  return screenshot || [
    "store.png",
    `${app.display_name} available from the signed Cobalt Apps Catalog`
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

// A listing with one thumbnail tells a visitor nothing about what the app is
// like to use. Any app that published extra captures under its own media
// directory gets them as a gallery; the file names are already descriptive
// enough to carry the alt text.
const galleryShots = id => {
  try {
    return readdirSync(resolve(root, "docs/media/site/apps", id))
      .filter(file => file.endsWith(".png"))
      .sort();
  } catch {
    return [];
  }
};
const shotCaption = file =>
  file
    .replace(/\.png$/, "")
    .replaceAll("-", " ")
    .replaceAll("_", " ");
const gallery = (app, name) => {
  const shots = galleryShots(app.id);
  if (shots.length < 2) return "";
  const figures = shots
    .map(
      file =>
        `      <figure><img src="../../media/site/apps/${app.id}/${file}" loading="lazy" alt="${name} on a Kobo Clara BW: ${escape(shotCaption(file))}"></figure>`
    )
    .join("\n");
  return `
  <section class="gallery" aria-label="${name} screenshots">
    <div class="strip">
${figures}
    </div>
  </section>`;
};
const whatsNew = app =>
  app.release_notes
    ? `
  <section class="whats-new">
    <h2>New in ${escape(app.version)}</h2>
    <p>${escape(app.release_notes)}</p>
  </section>`
    : "";
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
      <p class="eyebrow">${escape(categoryFor(app))}</p>
      <h1>${name}</h1>
      <p class="summary">${summary}</p>
      <dl class="facts">
        <div><dt>Version</dt><dd>${escape(app.version)}</dd></div>
        <div><dt>Permissions</dt><dd>${capabilities}</dd></div>
        <div><dt>Requires</dt><dd>Cobalt ${escape(app.minimum_cobalt_version)}</dd></div>
      </dl>
      <a class="cta" href="#setup-panel">Install on your Kobo</a>
    </div>
    <figure class="app-shot">
      <img src="../../media/site/apps/${screenshot}" width="1072" height="1448" alt="${escape(screenshotAlt)}">
    </figure>
  </div>${gallery(app, name)}${whatsNew(app)}${prerequisites}
  <section class="panel get-cobalt" id="setup-panel">
    <div class="get-cobalt-copy">
      <p class="eyebrow">Do not have Cobalt yet?</p>
      <h2>Install Cobalt directly from your browser</h2>
      <p>Plug your Kobo into this computer and the browser writes Cobalt across. About a minute, and no terminal. Applications after that arrive over Wi-Fi from the Cobalt Apps Catalog, with no cable.</p>
      <p class="fine">Works in Chrome, Edge and Opera. <a href="https://github.com/BandarLabs/Cobalt/blob/main/docs/DEVICES.md#device-support-matrix">Check your Kobo is supported</a>.</p>
    </div>
    <a class="get-cobalt-go" href="../../install/">Install Cobalt<span aria-hidden="true">&#8594;</span></a>
  </section>
  <section class="panel" id="pair-panel">
    <p class="eyebrow">Already have Cobalt?</p>
    <h2>Link your Kobo to install</h2>
    <p>On your Kobo, open <strong>App Store</strong> and tap the globe in the top bar. Choose <strong>Link browser</strong>, then scan the QR code or enter the pairing code and verification key it shows.</p>
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

// The landing page used to carry a hand-written excerpt of the catalog, which
// drifted until twenty-six shipped apps were missing from it. The grid is now
// derived from the same manifests the app pages come from, so an app that
// ships is an app the site shows.
const gridApps = [
  ...systemApps,
  ...[...catalog.apps].sort((a, b) => a.display_name.localeCompare(b.display_name))
];
const gridCard = app => {
  const [screenshot, screenshotAlt] = screenshotFor(app);
  return `      <div class="app">
        <a class="shot" href="apps/${app.id}/"><img src="media/site/apps/${screenshot}" width="1072" height="1448" loading="lazy" alt="${escape(screenshotAlt)}"></a>
        <h3><a href="apps/${app.id}/">${escape(app.display_name)}</a></h3>
        <p>${escape(app.summary)}</p>
      </div>`;
};
const indexPath = resolve(root, "docs/index.html");
const indexHtml = readFileSync(indexPath, "utf8");
const gridStart = "<!-- apps-grid:start -->";
const gridEnd = "<!-- apps-grid:end -->";
const startIndex = indexHtml.indexOf(gridStart);
const endIndex = indexHtml.indexOf(gridEnd);
if (startIndex === -1 || endIndex === -1 || endIndex < startIndex) {
  throw new Error(`docs/index.html is missing its ${gridStart} / ${gridEnd} markers`);
}
writeFileSync(
  indexPath,
  `${indexHtml.slice(0, startIndex + gridStart.length)}\n${gridApps
    .map(gridCard)
    .join("\n")}\n      ${indexHtml.slice(endIndex)}`
);

// The README's hand-written table had drifted to two Store apps out of
// thirty-four. This table is derived, so publishing an app lists it.
const storeApps = [...catalog.apps].sort((a, b) =>
  a.display_name.localeCompare(b.display_name)
);
const readmeCell = app => {
  const [screenshot, screenshotAlt] = screenshotFor(app);
  const href = `apps/${app.id}/README.md`;
  return `<td width="33%" valign="top"><a href="${href}"><img width="230" src="docs/media/site/apps/${screenshot}" alt="${escape(screenshotAlt)}"></a><br><b><a href="${href}">${escape(app.display_name)}</a></b><br>${escape(app.summary)}</td>`;
};
const readmeRows = [];
for (let index = 0; index < storeApps.length; index += 3) {
  const row = storeApps.slice(index, index + 3).map(readmeCell);
  while (row.length < 3) row.push("<td></td>");
  readmeRows.push(`<tr>\n${row.join("\n")}\n</tr>`);
}
const readmeTable = `<table>\n${readmeRows.join("\n")}\n</table>`;
const readmePath = resolve(root, "README.md");
const readmeText = readFileSync(readmePath, "utf8");
const storeStart = "<!-- store-apps:start -->";
const storeEnd = "<!-- store-apps:end -->";
const storeStartIndex = readmeText.indexOf(storeStart);
const storeEndIndex = readmeText.indexOf(storeEnd);
if (storeStartIndex === -1 || storeEndIndex === -1 || storeEndIndex < storeStartIndex) {
  throw new Error(`README.md is missing its ${storeStart} / ${storeEnd} markers`);
}
writeFileSync(
  readmePath,
  `${readmeText.slice(0, storeStartIndex + storeStart.length)}\n${readmeTable}\n${readmeText.slice(storeEndIndex)}`
);
