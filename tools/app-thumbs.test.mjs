import test from "node:test";
import assert from "node:assert/strict";
import { thumbName } from "./app-thumbs.mjs";

test("a replaced screenshot does not find the copies made from the old one", () => {
  const before = thumbName("launcher.png", Buffer.from("old capture"), 480);
  const after = thumbName("launcher.png", Buffer.from("new capture"), 480);

  assert.notEqual(before, after);
  assert.match(before, /^launcher-480w-[0-9a-f]{12}\.webp$/);
});

test("each width of the same screenshot gets its own copy", () => {
  const bytes = Buffer.from("capture");

  assert.notEqual(thumbName("store.png", bytes, 480), thumbName("store.png", bytes, 800));
});
