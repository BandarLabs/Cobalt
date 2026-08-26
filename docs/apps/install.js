"use strict";

const RELAY = "https://cobalt-install-relay.anandabhishek.workers.dev";
const STORAGE_KEY = "cobalt.install-link.v1";
const encoder = new TextEncoder();
const app = document.querySelector("main");
const appId = app.dataset.appId;
const setupPanel = document.querySelector("#setup-panel");
const pairPanel = document.querySelector("#pair-panel");
const installPanel = document.querySelector("#install-panel");
const pairForm = document.querySelector("#pair-form");
const pairCode = document.querySelector("#pair-code");
const pairStatus = document.querySelector("#pair-status");
const installStatus = document.querySelector("#install-status");
const installButton = document.querySelector("#install");
const forgetButton = document.querySelector("#forget");
const deviceName = document.querySelector("#device-name");
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const APP_ID = /^[a-z][a-z0-9-]{0,31}$/;
const BASE64URL = /^[A-Za-z0-9_-]+$/;

function validString(value, length) {
  return typeof value === "string" && value.length === length && BASE64URL.test(value);
}

function normalizedPending(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const entries = value.appId ? [[value.appId, value]] : Object.entries(value);
  return Object.fromEntries(entries.filter(([id, pending]) =>
    APP_ID.test(id)
    && !id.endsWith("-")
    && !id.includes("--")
    && pending
    && typeof pending === "object"
    && UUID.test(pending.commandId)
    && typeof pending.expiresAt === "string"
  ));
}

function validPrivateJwk(value) {
  return Boolean(value)
    && typeof value === "object"
    && !Array.isArray(value)
    && value.kty === "EC"
    && value.crv === "P-256"
    && typeof value.d === "string";
}

function connectionValue(value) {
  if (
    !value
    || typeof value !== "object"
    || Array.isArray(value)
    || !UUID.test(value.deviceId)
    || !validString(value.browserToken, 43)
    || !validString(value.publicKey, 122)
    || !validString(value.browserPublicKey, 122)
    || !validPrivateJwk(value.browserPrivateKey)
  ) {
    return null;
  }
  return {
    deviceId: value.deviceId,
    browserToken: value.browserToken,
    publicKey: value.publicKey,
    browserPublicKey: value.browserPublicKey,
    browserPrivateKey: value.browserPrivateKey,
    deviceName: typeof value.deviceName === "string" && value.deviceName.length <= 64
      ? value.deviceName || "Kobo reader"
      : "Kobo reader",
    pending: normalizedPending(value.pending)
  };
}

function connection() {
  try {
    return connectionValue(JSON.parse(localStorage.getItem(STORAGE_KEY)));
  } catch {
    return null;
  }
}

function setStatus(element, message, tone = "") {
  if (element.textContent === message && element.dataset.tone === tone) return;
  element.textContent = message;
  element.dataset.tone = tone;
}

function showConnection(value) {
  setupPanel.hidden = Boolean(value);
  pairPanel.hidden = Boolean(value);
  installPanel.hidden = !value;
  if (value) deviceName.textContent = value.deviceName;
}

function saveConnection(value) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
}

function pendingFor(value) {
  if (!value?.pending) return null;
  if (value.pending.appId) {
    return value.pending.appId === appId ? value.pending : null;
  }
  return value.pending[appId] || null;
}

function savePending(value, pending) {
  const latest = connection();
  if (latest && latest.deviceId !== value.deviceId) return value;
  const target = latest?.deviceId === value.deviceId ? latest : value;
  if (target.pending?.appId) target.pending = {};
  target.pending ||= {};
  target.pending[appId] = pending;
  saveConnection(target);
  return target;
}

function clearPending(value, commandId) {
  const latest = connection();
  if (latest && latest.deviceId !== value.deviceId) return;
  const target = latest?.deviceId === value.deviceId ? latest : value;
  if (target.pending?.appId) {
    if (target.pending.commandId === commandId) delete target.pending;
  } else if (target.pending?.[appId]?.commandId === commandId) {
    delete target.pending[appId];
  }
  saveConnection(target);
}

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

class ApiError extends Error {
  constructor(message, status, code) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

async function json(response) {
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new ApiError(
      body.error?.message || "The service could not complete this request.",
      response.status,
      body.error?.code
    );
  }
  return body;
}

async function claim(code, body) {
  return json(await fetch(`${RELAY}/v1/pairings/${encodeURIComponent(code)}/claim`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body)
  }));
}

function fragmentParams() {
  return new URLSearchParams(location.hash.slice(1));
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

async function signInstall(value, envelope) {
  const key = await crypto.subtle.importKey(
    "jwk",
    value.browserPrivateKey,
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"]
  );
  const message = encoder.encode(
    `cobalt-install-v2\n${value.deviceId}\n${envelope.ephemeral_public_key}\n${envelope.nonce}\n${envelope.ciphertext}`
  );
  return base64Url(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, message));
}

async function envelopeFor(value) {
  const deviceKey = await crypto.subtle.importKey(
    "spki",
    fromBase64Url(value.publicKey),
    { name: "ECDH", namedCurve: "P-256" },
    false,
    []
  );
  const ephemeral = await crypto.subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    true,
    ["deriveBits"]
  );
  const shared = await crypto.subtle.deriveBits(
    { name: "ECDH", public: deviceKey },
    ephemeral.privateKey,
    256
  );
  const material = await crypto.subtle.importKey("raw", shared, "HKDF", false, ["deriveKey"]);
  const key = await crypto.subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: new Uint8Array(),
      info: encoder.encode("cobalt-app-install-v1")
    },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt"]
  );
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = encoder.encode(JSON.stringify({
    version: 2,
    app_id: appId,
    request_id: base64Url(crypto.getRandomValues(new Uint8Array(16))),
    requested_at: Math.floor(Date.now() / 1000)
  }));
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv: nonce, additionalData: encoder.encode(value.deviceId) },
    key,
    plaintext
  );
  return {
    algorithm: "ECDH-P256-AES-256-GCM",
    ephemeral_public_key: base64Url(await crypto.subtle.exportKey("spki", ephemeral.publicKey)),
    nonce: base64Url(nonce),
    ciphertext: base64Url(ciphertext)
  };
}

async function queueInstall(value) {
  const envelope = await envelopeFor(value);
  const signature = await signInstall(value, envelope);
  return json(await fetch(`${RELAY}/v1/devices/${value.deviceId}/installs`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${value.browserToken}`,
      "content-type": "application/json"
    },
    body: JSON.stringify({ envelope, signature })
  }));
}

async function installState(value, commandId) {
  return json(await fetch(`${RELAY}/v1/devices/${value.deviceId}/installs/${commandId}`, {
    headers: { authorization: `Bearer ${value.browserToken}` }
  }));
}

function resultMessage(state) {
  if (state.state === "installed") {
    return {
      tone: "success",
      text: {
        installed: "Installed on your Kobo.",
        updated: "Updated on your Kobo.",
        "already-installed": "Already installed and up to date.",
        included: "This app is included with Cobalt."
      }[state.outcome] || "Completed on your Kobo."
    };
  }
  if (state.state === "failed") {
    if (state.failure === "expired") {
      return { tone: "warning", text: "The install request expired. Send it again." };
    }
    if (state.failure === "unavailable") {
      return { tone: "warning", text: "This app is not available in the current catalog. Nothing changed." };
    }
    return { tone: "error", text: "The install could not be completed. Open App Store on your Kobo and try again." };
  }
  if (!state.device_online) {
    return { tone: "warning", text: "Waiting for your Kobo — open Cobalt App Store to continue." };
  }
  return { tone: "", text: state.state === "installing" ? "Installing on your Kobo…" : "Sent to your Kobo…" };
}

async function watch(value, commandId) {
  for (;;) {
    try {
      const state = await installState(value, commandId);
      const message = resultMessage(state);
      setStatus(installStatus, message.text, message.tone);
      if (state.state === "installed" || state.state === "failed") {
        clearPending(value, commandId);
        return;
      }
      const delay = state.device_online ? 3000 : 30000;
      await new Promise(resolve => setTimeout(resolve, delay));
    } catch (error) {
      if (error.status === 404) {
        clearPending(value, commandId);
        throw error;
      }
      if (error.status === 401 || error.status === 403) throw error;
      setStatus(installStatus, "Status is temporarily unavailable. The request remains queued.", "warning");
      await new Promise(resolve => setTimeout(resolve, 30000));
    }
  }
}

pairForm.addEventListener("submit", async event => {
  event.preventDefault();
  const code = pairCode.value.replace(/[^A-Z0-9]/gi, "").toUpperCase();
  if (code.length !== 8) {
    setStatus(pairStatus, "Enter the 8-character code shown on your Kobo.", "error");
    return;
  }
  const button = pairForm.querySelector("button");
  button.disabled = true;
  setStatus(pairStatus, "Linking…");
  try {
    const fragment = fragmentParams();
    const secret = fragment.get("s");
    const browserKey = await generateBrowserKey();
    const claimed = await claim(code, {
      browser_public_key: browserKey.publicKey,
      ...secret && { proof: await pairProof(secret, browserKey.publicKey) }
    });
    if (
      typeof claimed.device_name !== "string"
      || claimed.device_name.length === 0
      || claimed.device_name.length > 64
    ) {
      throw new Error("The install service returned an invalid pairing response.");
    }
    const expected = fragment.get("k");
    if (expected && !(await deviceKeyMatchesFragment(claimed.device_public_key, expected))) {
      throw new Error("The install service returned a key that does not match your Kobo. Nothing was linked.");
    }
    const value = connectionValue({
      deviceId: claimed.device_id,
      browserToken: claimed.browser_token,
      publicKey: claimed.device_public_key,
      browserPublicKey: browserKey.publicKey,
      browserPrivateKey: browserKey.privateKey,
      deviceName: claimed.device_name
    });
    if (!value || typeof claimed.device_online !== "boolean") {
      throw new Error("The install service returned an invalid pairing response.");
    }
    saveConnection(value);
    showConnection(value);
    installPanel.querySelector("h2").tabIndex = -1;
    installPanel.querySelector("h2").focus();
    setStatus(
      installStatus,
      claimed.device_online ? "Ready to install." : "Linked. Open Cobalt App Store on your Kobo to receive installs.",
      claimed.device_online ? "success" : "warning"
    );
  } catch (error) {
    setStatus(pairStatus, error.message, "error");
  } finally {
    button.disabled = false;
  }
});

installButton.addEventListener("click", async () => {
  const value = connection();
  if (!value) return showConnection(null);
  const pending = pendingFor(value);
  if (pending) {
    installButton.disabled = true;
    try {
      await watch(value, pending.commandId);
    } finally {
      installButton.disabled = false;
    }
    return;
  }
  installButton.disabled = true;
  setStatus(installStatus, "Preparing encrypted request…");
  try {
    const queued = await queueInstall(value);
    const pending = {
      appId,
      commandId: queued.command_id,
      expiresAt: queued.expires_at
    };
    savePending(value, pending);
    const message = resultMessage(queued);
    setStatus(installStatus, message.text, message.tone);
    await watch(value, queued.command_id);
  } catch (error) {
    if (error.status === 401 || error.status === 403) {
      const latest = connection();
      if (!latest || latest.deviceId === value.deviceId) {
        localStorage.removeItem(STORAGE_KEY);
        showConnection(null);
        setStatus(pairStatus, "This browser is no longer linked. Link it again to continue.", "warning");
      }
    } else if (error.code === "device_not_found") {
      const latest = connection();
      if (!latest || latest.deviceId === value.deviceId) {
        localStorage.removeItem(STORAGE_KEY);
        showConnection(null);
        setStatus(pairStatus, "This Kobo link is no longer available. Link it again to continue.", "warning");
      }
    } else if (error.status === 404) {
      setStatus(installStatus, "The install request expired. Send it again.", "warning");
    } else if (error instanceof TypeError) {
      setStatus(installStatus, "The install service could not be reached. Check your connection and try again.", "error");
    } else {
      setStatus(installStatus, error.message, "error");
    }
  } finally {
    installButton.disabled = false;
  }
});

forgetButton.addEventListener("click", () => {
  localStorage.removeItem(STORAGE_KEY);
  pairCode.value = "";
  setStatus(pairStatus, "This browser is no longer linked.");
  showConnection(null);
});

const queryCode = new URLSearchParams(location.search).get("code");
if (queryCode) pairCode.value = queryCode.toUpperCase();
const saved = connection();
showConnection(saved);
const pending = pendingFor(saved);
if (saved && pending) {
  installButton.disabled = true;
  watch(saved, pending.commandId)
    .catch(error => {
      if (error.status === 401 || error.status === 403) {
        const latest = connection();
        if (!latest || latest.deviceId === saved.deviceId) {
          localStorage.removeItem(STORAGE_KEY);
          showConnection(null);
          setStatus(pairStatus, "This browser is no longer linked. Link it again to continue.", "warning");
        }
      } else if (error.status === 404) {
        setStatus(installStatus, "The install request expired. Send it again.", "warning");
      } else {
        setStatus(installStatus, error.message, "error");
      }
    })
    .finally(() => { installButton.disabled = false; });
}
