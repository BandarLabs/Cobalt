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
export function catalogReleaseTag({ eventName, baseRef, refName }) {
  const pullRequest =
    eventName === "pull_request" || eventName === "pull_request_target";
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
        refName: process.env.GITHUB_REF_NAME
      })
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
