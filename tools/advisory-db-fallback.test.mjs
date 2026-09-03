import test from "node:test";
import assert from "node:assert/strict";
import { fallbackFailure, fallbackRefusal } from "./advisory-db-fallback.mjs";

const SNAPSHOT = "309ad29d8fe448bf986019e05d47b9e0e29a2218";

function record(overrides = {}) {
  return {
    format_version: 1,
    snapshot: SNAPSHOT,
    expires: "2026-09-18",
    reason: "upstream shipped an unparseable advisory",
    ...overrides
  };
}

test("an unacknowledged fallback is refused", () => {
  assert.match(
    fallbackFailure(null, SNAPSHOT, "2026-09-04"),
    /no tools\/advisory-db-fallback\.json accepts auditing against a pinned snapshot/
  );
});

test("a dated acknowledgement of this snapshot is accepted until it expires", () => {
  assert.equal(fallbackFailure(record(), SNAPSHOT, "2026-09-04"), null);
  assert.equal(fallbackFailure(record(), SNAPSHOT, "2026-09-18"), null);
  assert.match(
    fallbackFailure(record(), SNAPSHOT, "2026-09-19"),
    /stopped accepting this snapshot on 2026-09-18, and today is 2026-09-19/
  );
});

test("moving the pin voids the acknowledgement of the old one", () => {
  const moved = "0".repeat(40);
  assert.match(
    fallbackFailure(record(), moved, "2026-09-04"),
    new RegExp(`accepts snapshot ${SNAPSHOT}, but this run pins ${moved}`)
  );
});

test("an acknowledgement without a reason, a date or a snapshot is refused", () => {
  assert.match(
    fallbackFailure(record({ reason: "  " }), SNAPSHOT, "2026-09-04"),
    /format_version 1 record with a reason/
  );
  assert.match(
    fallbackFailure(record({ format_version: 2 }), SNAPSHOT, "2026-09-04"),
    /format_version 1 record with a reason/
  );
  assert.match(
    fallbackFailure(record({ snapshot: "309ad29d" }), SNAPSHOT, "2026-09-04"),
    /names no advisory database snapshot/
  );
  assert.match(
    fallbackFailure(record({ expires: "soon" }), SNAPSHOT, "2026-09-04"),
    /has no expiry date/
  );
});

test("the refusal says what the blind spot is and how to accept it", () => {
  const message = fallbackRefusal("no record", SNAPSHOT);
  assert.match(message, /cannot see any\nadvisory published after it/);
  assert.match(message, /reports an audit nobody performed/);
  assert.match(message, new RegExp(`"snapshot": "${SNAPSHOT}"`));
  assert.match(message, /"expires": "YYYY-MM-DD"/);
});
