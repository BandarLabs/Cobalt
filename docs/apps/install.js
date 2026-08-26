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

function connection() {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY));
    return value?.deviceId && value?.browserToken && value?.publicKey ? value : null;
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

async function claim(code) {
  return json(await fetch(`${RELAY}/v1/pairings/${encodeURIComponent(code)}/claim`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}"
  }));
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
  const plaintext = encoder.encode(JSON.stringify({ version: 1, app_id: appId }));
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
  return json(await fetch(`${RELAY}/v1/devices/${value.deviceId}/installs`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${value.browserToken}`,
      "content-type": "application/json"
    },
    body: JSON.stringify({ envelope })
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
    const claimed = await claim(code);
    const value = {
      deviceId: claimed.device_id,
      browserToken: claimed.browser_token,
      publicKey: claimed.device_public_key,
      deviceName: claimed.device_name
    };
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
