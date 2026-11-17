import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { needsBuild, newestMtime } from "./launch.mjs";

function tmpdir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "muis-launch-"));
}

function write(file, body = "x", mtimeSeconds) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, body);
  if (mtimeSeconds !== undefined) fs.utimesSync(file, mtimeSeconds, mtimeSeconds);
}

test("a missing bundle always needs a build", () => {
  const dir = tmpdir();
  const dist = path.join(dir, "dist");
  const src = path.join(dir, "src");
  fs.mkdirSync(src, { recursive: true });
  assert.equal(needsBuild({ dist, sources: [src] }), true);
  fs.rmSync(dir, { recursive: true, force: true });
});

test("a bundle newer than its sources is reused", () => {
  const dir = tmpdir();
  const dist = path.join(dir, "dist");
  const src = path.join(dir, "src");
  write(path.join(src, "main.ts"), "x", 1_000);
  write(path.join(dist, "index.html"), "x", 2_000);
  assert.equal(needsBuild({ dist, sources: [src] }), false);
  fs.rmSync(dir, { recursive: true, force: true });
});

test("a source edit older than the bundle is ignored, newer is not", () => {
  const dir = tmpdir();
  const dist = path.join(dir, "dist");
  const src = path.join(dir, "src");
  write(path.join(dist, "index.html"), "x", 2_000);
  write(path.join(src, "a.ts"), "x", 1_500);
  assert.equal(needsBuild({ dist, sources: [src] }), false);
  write(path.join(src, "nested", "b.ts"), "x", 3_000);
  assert.equal(needsBuild({ dist, sources: [src] }), true);
  fs.rmSync(dir, { recursive: true, force: true });
});

test("newestMtime walks nested files and tolerates missing paths", () => {
  const dir = tmpdir();
  write(path.join(dir, "a", "b", "c.txt"), "x", 4_000);
  write(path.join(dir, "top.txt"), "x", 1_000);
  assert.equal(newestMtime(dir), 4_000_000);
  assert.equal(newestMtime(path.join(dir, "nope")), 0);
  fs.rmSync(dir, { recursive: true, force: true });
});
