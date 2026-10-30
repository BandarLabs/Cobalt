// Writes the WebP copies of the app screenshots that the landing page grid
// serves, and removes copies of screenshots that have since changed. It needs
// cwebp on the PATH, which CI does not have, so it is run by hand after adding
// or replacing a screenshot in docs/media/site/apps, before
// generate-app-pages.mjs. Until it is run, the grid shows that app's PNG.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, rmdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { screenshotsDir, thumbName, thumbWidths, thumbsDir } from "./app-thumbs.mjs";

// Most apps' grid screenshots sit flat in screenshotsDir, but an app may name
// one kept in its own folder, as Readeck's readeck/inbox.png is. Those are read
// from the grid the generator last wrote, so exactly what the grid shows is
// encoded and the per-app galleries beside them are not.
const gridScreenshots = () => {
  const index = readFileSync(resolve(screenshotsDir, "../../../index.html"), "utf8");
  const grid = index.slice(
    index.indexOf("<!-- apps-grid:start -->"),
    index.indexOf("<!-- apps-grid:end -->")
  );
  return [...grid.matchAll(/src="media\/site\/apps\/([^"]+\.png)"/g)].map(match => match[1]);
};

mkdirSync(thumbsDir, { recursive: true });
const current = new Set();
const screenshots = [
  ...new Set([
    ...readdirSync(screenshotsDir).filter(file => file.endsWith(".png")),
    ...gridScreenshots()
  ])
].sort();
for (const screenshot of screenshots) {
  const source = resolve(screenshotsDir, screenshot);
  const bytes = readFileSync(source);
  for (const width of thumbWidths) {
    const name = thumbName(screenshot, bytes, width);
    current.add(name);
    const output = resolve(thumbsDir, name);
    if (existsSync(output)) continue;
    mkdirSync(dirname(output), { recursive: true });
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
// Copies of screenshots that have changed or left the grid, in any folder,
// and then any folder left empty by removing them.
for (const entry of readdirSync(thumbsDir, { recursive: true, withFileTypes: true })) {
  if (!entry.isFile()) continue;
  const path = resolve(entry.parentPath, entry.name);
  const name = relative(thumbsDir, path).split("\\").join("/");
  if (!current.has(name)) {
    rmSync(path);
    console.log(`removed ${name}`);
  }
}
for (const entry of readdirSync(thumbsDir, { withFileTypes: true })) {
  const folder = resolve(thumbsDir, entry.name);
  if (entry.isDirectory() && readdirSync(folder).length === 0) rmdirSync(folder);
}
