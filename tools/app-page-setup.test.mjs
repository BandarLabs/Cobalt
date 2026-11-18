import test from "node:test";
import assert from "node:assert/strict";
import { setupPanel, validatedSetup } from "./app-page-setup.mjs";

test("apps without setup metadata keep the ordinary install page", () => {
  assert.equal(validatedSetup({ id: "plain" }), null);
  assert.equal(setupPanel({ id: "plain" }), "");
});

test("setup metadata renders escaped steps links and commands", () => {
  const html = setupPanel({
    id: "library",
    setup: {
      steps: [
        {
          text: "Create a read-only key.",
          link: { label: "Key settings", url: "https://example.com/settings/keys?a=1&b=2" }
        },
        {
          text: "Install the key under the exact secret name.",
          command: "kobo secret set library --device <address>"
        }
      ]
    }
  });

  assert.match(html, /Before you install/);
  assert.match(html, /https:\/\/example\.com\/settings\/keys\?a=1&amp;b=2/);
  assert.match(html, /kobo secret set library --device &lt;address&gt;/);
  assert.doesNotMatch(html, /<address>/);
});

test("steps that use commands say how to install the ready-built kobo command", () => {
  const html = setupPanel({
    id: "library",
    setup: { steps: [{ text: "Install the key.", command: "kobo secret set library" }] }
  });

  assert.match(html, /install\.sh \| sh -s -- --no-setup/);
  assert.match(html, /<code>kobo<\/code> command/);
});

test("steps without commands do not send readers to a terminal", () => {
  const html = setupPanel({
    id: "reader-only",
    setup: { steps: [{ text: "Open the app and sign in." }] }
  });

  assert.doesNotMatch(html, /install\.sh/);
});

test("setup metadata rejects unsafe links and unknown fields", () => {
  assert.throws(
    () =>
      validatedSetup({
        id: "unsafe",
        setup: {
          steps: [
            {
              text: "Open settings.",
              link: { label: "Settings", url: "http://example.com/settings" }
            }
          ]
        }
      }),
    /absolute HTTPS URL/
  );
  assert.throws(
    () => validatedSetup({ id: "unknown", setup: { steps: [{ text: "Prepare.", html: "<b>" }] } }),
    /unknown field 'html'/
  );
});

test("setup metadata keeps pages short and scannable", () => {
  assert.throws(
    () =>
      validatedSetup({
        id: "long",
        setup: { steps: Array.from({ length: 7 }, () => ({ text: "One step." })) }
      }),
    /between 1 and 6 entries/
  );
});
