"use strict";

const RELAY = "https://cobalt-install-relay.anandabhishek.workers.dev";
const STORAGE_KEY = "cobalt.install-link.v1";
const form = document.querySelector("#pair-form");
const input = document.querySelector("#pair-code");
const status = document.querySelector("#pair-status");
const next = document.querySelector("#continue");

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
    const claimed = await responseJson(await fetch(
      `${RELAY}/v1/pairings/${encodeURIComponent(code)}/claim`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{}"
      }
    ));
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      deviceId: claimed.device_id,
      browserToken: claimed.browser_token,
      publicKey: claimed.device_public_key,
      deviceName: claimed.device_name
    }));
    form.hidden = true;
    next.hidden = false;
    next.tabIndex = -1;
    next.focus();
    setStatus(
      claimed.device_online
        ? `${claimed.device_name} is linked.`
        : `${claimed.device_name} is linked. Open Cobalt App Store on your Kobo to receive installs.`,
      claimed.device_online ? "success" : "warning"
    );
  } catch (error) {
    setStatus(error.message, "error");
  } finally {
    button.disabled = false;
  }
});

const code = new URLSearchParams(location.search).get("code");
if (code) input.value = code.toUpperCase();
