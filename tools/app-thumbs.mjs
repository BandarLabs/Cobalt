import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The landing page draws each app's screenshot about 240 pixels wide, but the
// screenshots are captured at the Clara BW's full 1072. Forty-nine of them
// came to 3.4 MB for a grid that shows a fraction of those pixels, so each
// one also gets WebP copies at the widths the grid can use.
//
// A copy is named after the bytes of the PNG it was made from. A screenshot
// replaced without re-encoding then finds no copy, and the page falls back to
// the new PNG rather than showing the old screen.
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const screenshotsDir = resolve(root, "docs/media/site/apps");
export const thumbsDir = resolve(root, "docs/media/site/app-thumbs");
export const thumbWidths = [480, 800];

export const thumbName = (screenshot, bytes, width) => {
  const hash = createHash("sha256").update(bytes).digest("hex").slice(0, 12);
  return `${screenshot.replace(/\.png$/, "")}-${width}w-${hash}.webp`;
};

// The copies of a screenshot as it is now, as [width, file name] pairs, or
// null when any of them is missing.
export const thumbsFor = screenshot => {
  const bytes = readFileSync(resolve(screenshotsDir, screenshot));
  const thumbs = thumbWidths.map(width => [width, thumbName(screenshot, bytes, width)]);
  return thumbs.every(([, name]) => existsSync(resolve(thumbsDir, name))) ? thumbs : null;
};
