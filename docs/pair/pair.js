"use strict";

const RELAY = "https://cobalt-install-relay.anandabhishek.workers.dev";
const STORAGE_KEY = "cobalt.install-link.v1";
const form = document.querySelector("#pair-form");
const input = document.querySelector("#pair-code");
const status = document.querySelector("#pair-status");
const next = document.querySelector("#continue");
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const BASE64URL = /^[A-Za-z0-9_-]+$/;

function validString(value, length) {
  return typeof value === "string" && value.length === length && BASE64URL.test(value);
}

function pairingValue(value) {
  if (
    !value
    || typeof value !== "object"
    || Array.isArray(value)
    || !UUID.test(value.device_id)
    || !validString(value.browser_token, 43)
    || !validString(value.device_public_key, 122)
    || typeof value.device_online !== "boolean"
    || typeof value.device_name !== "string"
    || value.device_name.length === 0
    || value.device_name.length > 64
  ) {
    throw new Error("The install service returned an invalid pairing response.");
  }
  return {
    deviceId: value.device_id,
    browserToken: value.browser_token,
    publicKey: value.device_public_key,
    deviceName: value.device_name,
    deviceOnline: value.device_online
  };
}

function fragmentParams() {
  return new URLSearchParams(location.hash.slice(1));
}

const encoder = new TextEncoder();

function base64Url(bytes) {
  let binary = "";
  for (const byte of new Uint8Array(bytes)) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function fromBase64Url(value) {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  const binary = atob(padded);
  return Uint8Array.from(binary, character => character.charCodeAt(0));
}

async function generateBrowserKey() {
  const pair = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["sign"]
  );
  return {
    publicKey: base64Url(await crypto.subtle.exportKey("spki", pair.publicKey)),
    privateKey: await crypto.subtle.exportKey("jwk", pair.privateKey)
  };
}

async function deviceKeyMatchesFragment(publicKey, expected) {
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(publicKey));
  return base64Url(new Uint8Array(digest).slice(0, 16)) === expected;
}

async function pairProof(secret, browserPublicKey) {
  const key = await crypto.subtle.importKey(
    "raw",
    fromBase64Url(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  return base64Url(await crypto.subtle.sign(
    "HMAC",
    key,
    encoder.encode(`cobalt-pair-proof-v1\n${browserPublicKey}`)
  ));
}

function setStatus(message, tone = "") {
  if (status.textContent === message && status.dataset.tone === tone) return;
  status.textContent = message;
  status.dataset.tone = tone;
}

async function responseJson(response) {
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(body.error?.message || "The service could not complete this request.");
  }
  return body;
}

form.addEventListener("submit", async event => {
  event.preventDefault();
  const code = input.value.replace(/[^A-Z0-9]/gi, "").toUpperCase();
  if (code.length !== 8) {
    setStatus("Enter the 8-character code shown on your Kobo.", "error");
    return;
  }
  const button = form.querySelector("button");
  button.disabled = true;
  setStatus("Linking…");
  try {
    const fragment = fragmentParams();
    const secret = fragment.get("s");
    const browserKey = await generateBrowserKey();
    const claimed = pairingValue(await responseJson(await fetch(
      `${RELAY}/v1/pairings/${encodeURIComponent(code)}/claim`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          browser_public_key: browserKey.publicKey,
          ...secret && { proof: await pairProof(secret, browserKey.publicKey) }
        })
      }
    )));
    const expected = fragment.get("k");
    if (expected && !(await deviceKeyMatchesFragment(claimed.publicKey, expected))) {
      throw new Error("The install service returned a key that does not match your Kobo. Nothing was linked.");
    }
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      deviceId: claimed.deviceId,
      browserToken: claimed.browserToken,
      publicKey: claimed.publicKey,
      browserPublicKey: browserKey.publicKey,
      browserPrivateKey: browserKey.privateKey,
      deviceName: claimed.deviceName
    }));
    form.hidden = true;
    next.hidden = false;
    next.tabIndex = -1;
    next.focus();
    setStatus(
      claimed.deviceOnline
        ? `${claimed.deviceName} is linked.`
        : `${claimed.deviceName} is linked. Open Cobalt App Store on your Kobo to receive installs.`,
      claimed.deviceOnline ? "success" : "warning"
    );
  } catch (error) {
    setStatus(error.message, "error");
  } finally {
    button.disabled = false;
  }
});

const code = new URLSearchParams(location.search).get("code");
if (code) input.value = code.toUpperCase();
