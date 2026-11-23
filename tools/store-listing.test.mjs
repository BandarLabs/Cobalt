import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { listingGroups, listingPath, listingText, storeListing, tableEntries } from "./store-listing.mjs";

test("the listing the kobo command is built with is what the website's tables and pictures say now", () => {
  assert.equal(
    readFileSync(listingPath, "utf8"),
    listingText(storeListing()),
    "run node tools/store-listing.mjs and commit crates/kobo-cli/src/ui/store-listing.json"
  );
});

test("every listed picture has a name the companion will agree to fetch, and a caption", () => {
  const listing = storeListing();
  const shots = Object.values(listing.shots).flat();

  assert.ok(shots.length > 100);
  for (const shot of shots) {
    assert.match(shot.name, /^[a-z0-9][a-z0-9-]*(\/[a-z0-9][a-z0-9-]*)?\.png$/);
    assert.ok(shot.caption.trim().length > 0, shot.name);
    assert.ok(shot.width > 0 && shot.height > 0, shot.name);
  }
  assert.equal(new Set(shots.map(shot => shot.name)).size, shots.length);
});

test("every app is in exactly one group and the groups keep the website's order", () => {
  const listing = storeListing();
  const listed = listing.groups.flatMap(group => group.apps);

  assert.deepEqual(listing.groups.map(group => group.id), ["reading", "games", "everyday", "home-and-computer", "developers"]);
  assert.equal(new Set(listed).size, listed.length);
  assert.deepEqual([...listed].sort(), Object.keys(listing.shots).sort());
});

test("a picture the folder repeats is listed once, under the caption written for it", () => {
  const backgammon = storeListing().shots.backgammon;

  // backgammon/backgammon-match.png is the leading picture again.
  assert.deepEqual(backgammon.map(shot => shot.name), [
    "backgammon.png",
    "backgammon/backgammon-selected.png",
    "backgammon/backgammon-opening.png",
  ]);
  assert.equal(backgammon[0].caption, "The computer's opening move marked on the board");
  assert.equal(backgammon[1].caption, "A checker picked up, with the points it can reach boxed");
});

test("a table is read with bare and quoted keys, and a line it cannot read is an error", () => {
  const source = 'const things = {\n  // why\n  plain: "one",\n  "with-hyphen": ["a.png", "A, with a comma"],\n  gone: null\n};\n';

  assert.deepEqual(tableEntries(source, "things"), {
    plain: "one",
    "with-hyphen": ["a.png", "A, with a comma"],
    gone: null,
  });
  assert.throws(() => tableEntries(source, "others"), /read the others table/);
  assert.throws(() => tableEntries("const things = {\n  plain: one(),\n};\n", "things"));
  assert.throws(() => tableEntries("const things = {\n  ...spread\n};\n", "things"), /read this line/);
});

test("the groups are read in order with the categories each takes in", () => {
  const source = 'const listingGroups = [\n  { id: "a", title: "First", categories: ["One", "Two"] },\n  { id: "b", title: "Second", categories: ["Three"] }\n];\n';

  assert.deepEqual(listingGroups(source), [
    { id: "a", title: "First", categories: ["One", "Two"] },
    { id: "b", title: "Second", categories: ["Three"] },
  ]);
  assert.throws(() => listingGroups("const listingGroups = [\n  group()\n];\n"), /read this line/);
});
