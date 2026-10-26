// Writes the WebP copies of the app screenshots that the landing page grid
// serves, and removes copies of screenshots that have since changed. It needs
// cwebp on the PATH, which CI does not have, so it is run by hand after adding
// or replacing a screenshot in docs/media/site/apps, before
// generate-app-pages.mjs. Until it is run, the grid shows that app's PNG.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { screenshotsDir, thumbName, thumbWidths, thumbsDir } from "./app-thumbs.mjs";

mkdirSync(thumbsDir, { recursive: true });
const current = new Set();
const screenshots = readdirSync(screenshotsDir).filter(file => file.endsWith(".png")).sort();
for (const screenshot of screenshots) {
  const source = resolve(screenshotsDir, screenshot);
  const bytes = readFileSync(source);
  for (const width of thumbWidths) {
    const name = thumbName(screenshot, bytes, width);
    current.add(name);
    const output = resolve(thumbsDir, name);
    if (existsSync(output)) continue;
    // Quality 80 keeps small type and one-pixel rules clean at these widths
    // and comes to about a quarter of the PNG.
    const result = spawnSync(
      "cwebp",
      ["-quiet", "-q", "80", "-m", "6", "-resize", String(width), "0", source, "-o", output],
      { stdio: "inherit" }
    );
    if (result.error?.code === "ENOENT") {
      throw new Error("cwebp is not on the PATH; it comes with libwebp");
    }
    if (result.status !== 0) throw new Error(`cwebp could not encode ${screenshot}`);
    console.log(`wrote ${name}`);
  }
}
for (const name of readdirSync(thumbsDir)) {
  if (!current.has(name)) {
    rmSync(resolve(thumbsDir, name));
    console.log(`removed ${name}`);
  }
}
