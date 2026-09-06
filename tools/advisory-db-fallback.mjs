import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const RECORD_PATH = "tools/advisory-db-fallback.json";
const SNAPSHOT_PATTERN = /^[0-9a-f]{40}$/;

// Expiry is decided by comparing two date strings, which orders them by date
// only while both are real dates written the one way. `2026-13-45` sorts after
// every genuine date and would never expire, so a date that does not exist is
// not a date here.
export function isCalendarDate(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value ?? "")) return false;
  const parsed = new Date(`${value}T00:00:00Z`);
  return (
    Number.isFinite(parsed.getTime()) &&
    parsed.toISOString().slice(0, 10) === value
  );
}

// Auditing against a pinned snapshot answers a question nobody asked: it is
// blind to every advisory published after the pin, and it reports that as a
// clean audit. It is still the right thing to do while the upstream database
// genuinely will not load, so the blind spot is allowed, named and dated, and
// it stops being allowed on the date it says.
export function fallbackFailure(record, snapshot, today) {
  if (record === null) {
    return `no ${RECORD_PATH} accepts auditing against a pinned snapshot`;
  }
  if (
    record?.format_version !== 1 ||
    typeof record.reason !== "string" ||
    record.reason.trim().length === 0
  ) {
    return `${RECORD_PATH} is not a format_version 1 record with a reason`;
  }
  if (!SNAPSHOT_PATTERN.test(record.snapshot || "")) {
    return `${RECORD_PATH} names no advisory database snapshot`;
  }
  if (record.snapshot !== snapshot) {
    return `${RECORD_PATH} accepts snapshot ${record.snapshot}, but this run pins ${snapshot}`;
  }
  if (!isCalendarDate(record.expires)) {
    return `${RECORD_PATH} has no expiry date`;
  }
  if (record.expires < today) {
    return `${RECORD_PATH} stopped accepting this snapshot on ${record.expires}, and today is ${today}`;
  }
  return null;
}

export function fallbackRefusal(failure, snapshot) {
  return [
    "The current RustSec advisory database failed to load, so this run can only",
    `audit against the snapshot pinned at ${snapshot}. That snapshot cannot see any`,
    "advisory published after it, and passing on it reports an audit nobody performed.",
    "",
    `Refusing because ${failure}.`,
    "",
    `To accept that blind spot for a bounded time, commit ${RECORD_PATH}:`,
    "",
    "  {",
    '    "format_version": 1,',
    `    "snapshot": "${snapshot}",`,
    '    "expires": "YYYY-MM-DD",',
    '    "reason": "why the upstream database will not load"',
    "  }",
    "",
    "Remove it as soon as upstream loads again; it expires on its own either way."
  ].join("\n");
}

export function argumentsFrom(argv) {
  const allowed = ["--record", "--snapshot", "--today"];
  const usage =
    "usage: node tools/advisory-db-fallback.mjs --record PATH --snapshot SHA --today YYYY-MM-DD";
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.includes(flag) || !value) throw new Error(usage);
    values.set(flag, value);
  }
  if (values.size !== allowed.length) throw new Error(usage);
  // The shapes the usage line describes are the shapes the comparisons below
  // assume, and nothing else checks them. A malformed date silently decides the
  // expiry the wrong way round, which is the one answer this tool exists to get
  // right, so say so here rather than audit blind on a typo.
  const snapshot = values.get("--snapshot");
  if (!SNAPSHOT_PATTERN.test(snapshot)) {
    throw new Error(
      `--snapshot ${snapshot} is not a 40-character advisory database commit`
    );
  }
  const today = values.get("--today");
  if (!isCalendarDate(today)) {
    throw new Error(`--today ${today} is not a date written YYYY-MM-DD`);
  }
  return values;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const values = argumentsFrom(process.argv.slice(2));
    const snapshot = values.get("--snapshot");
    let record = null;
    try {
      record = JSON.parse(readFileSync(values.get("--record"), "utf8"));
    } catch {
      record = null;
    }
    const failure = fallbackFailure(record, snapshot, values.get("--today"));
    if (failure) {
      console.error(fallbackRefusal(failure, snapshot));
      process.exitCode = 1;
    } else {
      console.log(
        `Auditing against pinned snapshot ${snapshot}, accepted until ${record.expires}: ${record.reason}`
      );
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
