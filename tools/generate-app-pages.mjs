import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const catalog = JSON.parse(readFileSync(resolve(root, "apps/catalog.json"), "utf8"));
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
const appIds = new Set(catalog.apps.map(app => app.id));

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
<header><div class="wrap"><a class="brand" href="../../">Cobalt</a><a class="back" href="../../#apps">All apps</a></div></header>
<main class="wrap" data-app-id="${id}">
  <p class="eyebrow">Kobo app</p>
  <h1>${name}</h1>
  <p class="summary">${summary}</p>
  <div class="meta"><span>Version ${escape(app.version)}</span><span>${capabilities}</span></div>
  <section class="panel setup" id="setup-panel">
    <p class="eyebrow">First-time setup</p>
    <h2>New to Cobalt? Set it up once.</h2>
    <p>Cobalt adds a secure app platform without replacing the Kobo reader. The first setup uses a USB cable; after that, apps install and update over Wi-Fi.</p>
    <ol>
      <li>Check that your Kobo model and firmware are supported.</li>
      <li>Connect the charged Kobo to a Mac or Linux computer and run the guided setup.</li>
      <li>Restart, open Cobalt, and return here. You will not need the cable for future apps.</li>
    </ol>
    <a class="button-link" href="https://github.com/BandarLabs/Cobalt/blob/main/docs/INSTALL.md">View the setup guide</a>
  </section>
  <section class="panel" id="pair-panel">
    <p class="eyebrow">Already using Cobalt?</p>
    <h2>Link your Kobo</h2>
    <p>On your Kobo, open <strong>App Store</strong>, then <strong>Install links</strong>. Scan the QR code or enter the code shown there.</p>
    <form id="pair-form">
      <div class="field">
        <label for="pair-code">Pairing code</label>
        <input id="pair-code" name="code" inputmode="text" autocomplete="one-time-code" maxlength="8" required>
      </div>
      <button type="submit">Link Kobo</button>
    </form>
    <p class="status" id="pair-status" role="status" aria-live="polite"></p>
    <p class="privacy">No account is required. The install request is encrypted in this browser and can only be opened by the linked Kobo.</p>
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

const home = readFileSync(resolve(root, "docs/index.html"), "utf8");
for (const app of catalog.apps) {
  if (!home.includes(`href="apps/${app.id}/"`)) {
    throw new Error(`docs/index.html does not link to app page: ${app.id}`);
  }
}
