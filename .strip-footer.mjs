// Scratch helper (never committed): remove the vendor footer from a PR body.
import { execFileSync } from "node:child_process";

const number = process.argv[2];
const body = execFileSync("gh", ["pr", "view", number, "--json", "body", "--jq", ".body"], {
  encoding: "utf8"
});

let stripped = body
  // The whole marker-delimited vendor block.
  .replace(/\n*<!-- codesmith:footer -->[\s\S]*?<!-- \/codesmith:footer -->\n*/g, "\n")
  // Any stray "Made with"/"Generated with" promotional line.
  .replace(/^.*(?:🤖\s*)?(?:generated|made|created|co-authored)\s+with\s+\[?[A-Za-z].*$/gim, "")
  .replace(/\n{3,}/g, "\n\n")
  .trimEnd();

// A trailing horizontal rule left behind by the removed block.
stripped = stripped.replace(/\n+-{3,}\s*$/, "").trimEnd() + "\n";

process.stdout.write(stripped);
