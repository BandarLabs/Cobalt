import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// What `kobo ui` needs to show the apps as the website does, and cannot get
// from the app manifests it is already built with: which group each app is
// listed under, which screens it has pictures of, and what each picture
// shows. The pictures themselves are far too large to build into a command,
// so the companion fetches them from the published site when they are looked
// at; this is the small part it carries so that it knows what to ask for and
// can draw the whole store, pictures aside, with no network at all.
//
// The groups and the leading picture of each app are tables inside
// generate-app-pages.mjs, which draws the website from them. They are read
// from its source here rather than moved somewhere both could import, so the
// website's generator stays the one place a listing is edited.

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const generator = resolve(root, "tools/generate-app-pages.mjs");
export const listingPath = resolve(root, "crates/kobo-cli/src/ui/store-listing.json");
const picturesDir = resolve(root, "docs/media/site/apps");

// The entries of one `const name = { ... };` table whose values are JSON.
// A line that is not a comment, a blank or `key: value,` is an error, because
// a table that has grown a shape this cannot read would otherwise lose
// entries without anybody being told.
export function tableEntries(source, name) {
  const found = new RegExp(`^const ${name} = \\{\\n([\\s\\S]*?)\\n\\};`, "m").exec(source);
  if (!found) throw new Error(`read the ${name} table from generate-app-pages.mjs`);
  const entries = {};
  for (const line of found[1].split("\n")) {
    if (line.trim() === "" || line.trim().startsWith("//")) continue;
    const entry = /^\s*(?:"([^"]+)"|([A-Za-z_]\w*)):\s*(.+?),?\s*$/.exec(line);
    if (!entry) throw new Error(`read this line of the ${name} table: ${line.trim()}`);
    entries[entry[1] ?? entry[2]] = JSON.parse(entry[3]);
  }
  return entries;
}

// The groups the website lists apps under, in its order.
export function listingGroups(source) {
  const found = /^const listingGroups = \[\n([\s\S]*?)\n\];/m.exec(source);
  if (!found) throw new Error("read the listingGroups table from generate-app-pages.mjs");
  return found[1].split("\n").map(line => {
    const group = /^\s*\{ id: ("[^"]+"), title: ("[^"]+"), categories: (\[[^\]]*\]) \},?$/.exec(line);
    if (!group) throw new Error(`read this line of the listingGroups table: ${line.trim()}`);
    return { id: JSON.parse(group[1]), title: JSON.parse(group[2]), categories: JSON.parse(group[3]) };
  });
}

// A PNG says how large it is at a fixed place near its start.
function pngSize(path) {
  const bytes = readFileSync(path);
  if (bytes.length < 24 || bytes.readUInt32BE(0) !== 0x89504e47) throw new Error(`${path} is not a PNG`);
  return { bytes, width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

// A file name read as a caption, for a picture nobody has written one for.
function captionFrom(file) {
  const words = file.replace(/\.png$/, "").replace(/^\d+[-_]/, "").replaceAll("-", " ").replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

// The pictures of one app: the one the website leads with, then the rest of
// its folder in the order the website's gallery shows them.
//
// Only the screens are listed. The few photographs of a reader that some
// folders hold are a different shape and a different kind of picture, and a
// store that frames every picture as a screen would frame those wrongly.
// A folder often holds a second copy of the leading picture, which is listed
// once.
function shotsFor(id, lead) {
  const folder = resolve(picturesDir, id);
  const captionsPath = resolve(folder, "captions.json");
  const captions = existsSync(captionsPath) ? JSON.parse(readFileSync(captionsPath, "utf8")) : {};
  const order = Object.keys(captions);
  const rank = file => (order.includes(file) ? order.indexOf(file) : order.length);
  const files = existsSync(folder)
    ? readdirSync(folder).filter(file => file.endsWith(".png")).sort().sort((a, b) => rank(a) - rank(b))
    : [];

  const shots = [];
  let leading = null;
  if (lead) {
    const [name, caption] = lead;
    leading = pngSize(resolve(picturesDir, name));
    shots.push({ name, caption, width: leading.width, height: leading.height });
  }
  for (const file of files) {
    const name = `${id}/${file}`;
    if (lead && name === lead[0]) continue;
    const picture = pngSize(resolve(folder, file));
    if (leading && picture.bytes.equals(leading.bytes)) {
      // The copy in the folder is the one somebody captioned.
      if (captions[file]) shots[0].caption = captions[file];
      continue;
    }
    shots.push({ name, caption: captions[file] ?? captionFrom(file), width: picture.width, height: picture.height });
  }
  return shots;
}

export function storeListing(source = readFileSync(generator, "utf8")) {
  const categories = tableEntries(source, "categories");
  const leads = tableEntries(source, "screenshots");
  const ids = Object.keys(categories).sort();
  const groups = listingGroups(source).map(group => ({
    id: group.id,
    title: group.title,
    apps: ids.filter(id => group.categories.includes(categories[id])),
  }));
  const grouped = new Set(groups.flatMap(group => group.apps));
  for (const id of ids) {
    if (!grouped.has(id)) throw new Error(`${id}'s category, ${categories[id]}, belongs to no listing group`);
  }
  return {
    groups,
    shots: Object.fromEntries(ids.map(id => [id, shotsFor(id, leads[id])])),
    release_note_overrides: tableEntries(source, "releaseNoteOverrides"),
  };
}

export const listingText = listing => `${JSON.stringify(listing, null, 2)}\n`;

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  writeFileSync(listingPath, listingText(storeListing()));
}
