import test from "node:test";
import assert from "node:assert/strict";
import { catalogReleaseTag } from "./app-catalog-channel.mjs";

test("a pull request is checked against the catalog its base branch publishes", () => {
  assert.equal(
    catalogReleaseTag({
      eventName: "pull_request",
      baseRef: "beta",
      refName: "1234/merge"
    }),
    "app-catalog-beta"
  );
  assert.equal(
    catalogReleaseTag({
      eventName: "pull_request",
      baseRef: "main",
      refName: "1234/merge"
    }),
    "app-catalog"
  );
});

test("a push is checked against the catalog its own branch publishes", () => {
  assert.equal(
    catalogReleaseTag({ eventName: "push", refName: "beta" }),
    "app-catalog-beta"
  );
  assert.equal(
    catalogReleaseTag({ eventName: "push", refName: "main" }),
    "app-catalog"
  );
  assert.equal(
    catalogReleaseTag({ eventName: "push", refName: "refs/heads/main" }),
    "app-catalog"
  );
});

test("branches outside the release channels are checked against beta", () => {
  assert.equal(
    catalogReleaseTag({ eventName: "push", refName: "app/backgammon" }),
    "app-catalog-beta"
  );
  assert.equal(
    catalogReleaseTag({ eventName: "workflow_dispatch", refName: "fix/wifi" }),
    "app-catalog-beta"
  );
});

test("an event that names no branch refuses to guess a channel", () => {
  assert.throws(
    () => catalogReleaseTag({ eventName: "pull_request", baseRef: "", refName: "1/merge" }),
    /pull_request names no branch/
  );
  assert.throws(
    () => catalogReleaseTag({ eventName: "push", refName: undefined }),
    /push names no branch/
  );
});
