import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The pre-merge check and the post-merge publication have to read the same
// published catalog. When they disagree, an app the smaller catalog does not
// list yet is skipped entirely before the merge and rejected immediately after
// it, so the pull request goes green and the merge goes red.
const CHANNELS = new Map([
  ["main", "app-catalog"],
  ["beta", "app-catalog-beta"]
]);

function branchName(ref) {
  if (typeof ref !== "string") return "";
  return ref.trim().replace(/^refs\/heads\//, "");
}

// A pull request is checked against the catalog its base branch publishes, not
// the branch it happens to be built from. Branches that are neither release
// channel are integration work headed for beta, and beta lists every app stable
// lists, so reading beta can only ever check more than stable would.
export function catalogReleaseTag({ eventName, baseRef, refName, refType }) {
  const pullRequest =
    eventName === "pull_request" || eventName === "pull_request_target";
  // A tag is not a branch, and the ref name of one carries no hint of the
  // channel it was cut from. Falling through to the default would answer beta
  // for a stable release tag and check the tagged tree against a base revision
  // from another line of history.
  if (!pullRequest && refType !== undefined && refType !== "branch") {
    throw new Error(
      `${eventName || "this event"} is on a ${refType}, and only a branch names an app catalog channel`
    );
  }
  const target = branchName(pullRequest ? baseRef : refName);
  if (!target) {
    throw new Error(
      `${eventName || "this event"} names no branch to resolve an app catalog channel from`
    );
  }
  return CHANNELS.get(target) || "app-catalog-beta";
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(
      catalogReleaseTag({
        eventName: process.env.GITHUB_EVENT_NAME,
        baseRef: process.env.GITHUB_BASE_REF,
        refName: process.env.GITHUB_REF_NAME,
        refType: process.env.GITHUB_REF_TYPE
      })
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
