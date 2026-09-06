#!/usr/bin/env node

import { writeFileSync } from "node:fs";
import { collectRegistry } from "./app-registry.mjs";

const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== "--out") {
  throw new Error("usage: node tools/collect-app-registry.mjs --out PATH");
}
writeFileSync(args[1], `${JSON.stringify(collectRegistry(), null, 2)}\n`);
console.log(`wrote ${args[1]}`);
