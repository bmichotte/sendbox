// The release workflow runs `node scripts/check-release-tag.mjs <tag>` from
// the repository root before building anything. These tests run it the same
// way, inside a throwaway Cargo workspace shaped like ours.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";

const script = join(import.meta.dirname, "check-release-tag.mjs");
const workspaces = [];

after(() => {
  for (const dir of workspaces) rmSync(dir, { recursive: true, force: true });
});

/// A workspace whose app crate inherits `version` from `[workspace.package]`.
function workspaceAtVersion(version) {
  const root = mkdtempSync(join(tmpdir(), "sendbox-release-"));
  workspaces.push(root);
  writeFileSync(
    join(root, "Cargo.toml"),
    `[workspace]
resolver = "3"
members = ["crates/sendbox-app"]

[workspace.package]
version = "${version}"
edition = "2024"
`,
  );
  const app = join(root, "crates", "sendbox-app");
  mkdirSync(join(app, "src"), { recursive: true });
  writeFileSync(
    join(app, "Cargo.toml"),
    `[package]
name = "sendbox-app"
version.workspace = true
edition.workspace = true
`,
  );
  writeFileSync(join(app, "src", "main.rs"), "fn main() {}\n");
  return root;
}

function checkReleaseTag(root, tag) {
  return spawnSync(process.execPath, [script, tag], {
    cwd: root,
    encoding: "utf8",
  });
}

test("accepts a tag that names the app version", () => {
  const root = workspaceAtVersion("1.4.2");

  const run = checkReleaseTag(root, "v1.4.2");

  assert.equal(run.status, 0, run.stderr);
});

test("rejects a tag that differs from the app version, naming both", () => {
  const root = workspaceAtVersion("1.4.2");

  const run = checkReleaseTag(root, "v1.5.0");

  assert.notEqual(run.status, 0);
  assert.match(run.stderr, /v1\.5\.0/);
  assert.match(run.stderr, /1\.4\.2/);
});
