#!/usr/bin/env node
// Usage: node scripts/check-release-tag.mjs <tag>
//
// Fails unless <tag> is `v` followed by the version Tauri bundles into the
// app: the `sendbox-app` crate's version, which it inherits from
// `[workspace.package]` in the root Cargo.toml. Run from the repository root.

import { execFileSync } from "node:child_process";

const tag = process.argv[2];

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    encoding: "utf8",
  }),
);
const { version } = metadata.packages.find((p) => p.name === "sendbox-app");

if (tag !== `v${version}`) {
  console.error(
    `Tag ${tag} does not match the app version ${version}: ` +
      `set [workspace.package] version in Cargo.toml, or tag v${version}.`,
  );
  process.exit(1);
}
