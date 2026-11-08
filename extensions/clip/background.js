// Send to Kobo: the right-click menu, and the hand-off to Cobalt's kobo
// command on this computer, which makes the article and sends it.
//
// The extension reads the page as the owner sees it and nothing else. It
// has no network access of its own and keeps no record of what was sent.

const api = globalThis.browser ?? globalThis.chrome;

// The host `kobo clip install` registers with the browser.
const HOST = "io.github.bandarlabs.cobalt_clip";

const MENUS = [
  { id: "send-page", title: "Send page to Kobo", contexts: ["page"] },
  { id: "send-link", title: "Send link to Kobo", contexts: ["link"] },
  { id: "send-selection", title: "Send selection to Kobo", contexts: ["selection"] }
];

// Menus are made again on every start: a browser that restarted an event
// page without them would show none, and one that kept them would refuse
// a second item with the same id.
function makeMenus() {
  api.contextMenus.removeAll(() => {
    for (const menu of MENUS) api.contextMenus.create(menu);
  });
}

api.runtime.onInstalled.addListener(makeMenus);
api.runtime.onStartup.addListener(makeMenus);

// Runs inside the page. It must stand alone: the browser copies its source
// into the tab, so nothing outside it is in reach.
function capturePage(wantSelection) {
  const HIDDEN = "data-send-to-kobo-hidden";
  const live = document.documentElement;
  const marked = [];
  for (const element of live.querySelectorAll("body *")) {
    const style = getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden" || element.hidden) {
      element.setAttribute(HIDDEN, "");
      marked.push(element);
    }
  }
  const copy = live.cloneNode(true);
  for (const element of marked) element.removeAttribute(HIDDEN);

  // Pictures as the page shows them now: a lazy picture's real address is
  // in currentSrc once it has loaded, and in a data attribute before.
  const liveImages = live.querySelectorAll("img");
  const copiedImages = copy.querySelectorAll("img");
  copiedImages.forEach((image, index) => {
    const shown = liveImages[index];
    const lazy = ["data-src", "data-lazy-src", "data-original", "data-url", "data-hi-res-src"]
      .map(name => image.getAttribute(name))
      .find(value => value && !value.startsWith("data:"));
    const current = shown && shown.currentSrc;
    const source = current && !current.startsWith("data:") ? current : lazy;
    if (source) {
      image.setAttribute("src", new URL(source, document.baseURI).href);
      image.removeAttribute("srcset");
      image.removeAttribute("sizes");
    }
  });
  for (const source of copy.querySelectorAll("picture source")) source.remove();
  for (const element of copy.querySelectorAll(`[${HIDDEN}], script, template`)) element.remove();

  let selection = "";
  const chosen = getSelection();
  if (wantSelection && chosen && chosen.rangeCount > 0) {
    const box = document.createElement("div");
    for (let index = 0; index < chosen.rangeCount; index += 1) {
      box.appendChild(chosen.getRangeAt(index).cloneContents());
    }
    selection = box.innerHTML;
  }
  return {
    url: location.href,
    title: document.title,
    html: "<!DOCTYPE html>\n" + copy.outerHTML,
    selection
  };
}

function notify(id, title, message) {
  // Cleared and made again rather than updated, which every browser here
  // can do.
  api.notifications.clear(id, () => {
    api.notifications.create(id, {
      type: "basic",
      iconUrl: api.runtime.getURL("icons/icon-128.png"),
      title,
      message
    });
  });
}

const NOT_SET_UP =
  "Cobalt's kobo command is not set up for this browser. In a terminal, run kobo clip install.";

function send(ask) {
  const id = `send-to-kobo-${Date.now()}`;
  notify(id, "Sending to your Kobo", ask.title || ask.url);
  let finished = false;
  let port;
  try {
    port = api.runtime.connectNative(HOST);
  } catch {
    notify(id, "Could not send to your Kobo", NOT_SET_UP);
    return;
  }
  port.onMessage.addListener(reply => {
    if (reply.type !== "done") return;
    finished = true;
    if (reply.outcome === "landed") {
      notify(id, "Landed on your Kobo", reply.title);
    } else if (reply.outcome === "waiting") {
      notify(id, "Waiting for your Kobo", reply.message);
    } else {
      notify(id, "Could not send to your Kobo", reply.message);
    }
    port.disconnect();
  });
  port.onDisconnect.addListener(() => {
    if (!finished) notify(id, "Could not send to your Kobo", NOT_SET_UP);
  });
  port.postMessage(ask);
}

function isWebPage(url) {
  return /^https?:\/\//i.test(url || "");
}

api.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "send-link") {
    if (!isWebPage(info.linkUrl)) {
      notify(`send-to-kobo-${Date.now()}`, "Could not send to your Kobo", "Only links to web pages can be sent.");
      return;
    }
    send({ url: info.linkUrl, title: info.linkText || info.selectionText || "" });
    return;
  }
  const url = info.pageUrl || (tab && tab.url);
  if (!tab || !isWebPage(url)) {
    notify(`send-to-kobo-${Date.now()}`, "Could not send to your Kobo", "Only web pages can be sent.");
    return;
  }
  api.scripting
    .executeScript({
      target: { tabId: tab.id },
      func: capturePage,
      args: [info.menuItemId === "send-selection"]
    })
    .then(results => {
      const page = results && results[0] && results[0].result;
      // A page the browser will not let extensions read is fetched by the
      // computer instead, as a link would be.
      send(page || { url, title: tab.title || "" });
    })
    .catch(() => send({ url, title: tab.title || "" }));
});
