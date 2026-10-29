import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";

// These are direct build/runner inputs; Tailwind additionally scans source files.
const fixed = [
  "package.json",
  "pnpm-lock.yaml",
  "postcss.config.cjs",
  "tailwind.config.cjs",
  "tsconfig.json",
  "tsconfig.node.json",
  "src/index.html",
  "tests/browser/vite.config.mjs",
  "tests/browser/screenshot-source.mjs",
  "tests/browser/run-unified-accounts.mjs",
  "tests/browser/unified-account-locators.mjs",
  "tests/browser/unified-accounts.html",
  "tests/browser/zcode-provider.html",
  "tests/browser/applications-c2.html",
];
function safePath(root, relative) {
  if (
    typeof relative !== "string" ||
    !relative ||
    path.isAbsolute(relative) ||
    relative.split(/[\\/]/).includes("..")
  )
    throw new Error("Invalid screenshot source path");
  const absolute = path.resolve(root, relative);
  if (!absolute.startsWith(`${root}${path.sep}`))
    throw new Error("Screenshot source escapes checkout");
  let current = root;
  for (const part of relative.split("/")) {
    current = path.join(current, part);
    if (existsSync(current) && lstatSync(current).isSymbolicLink())
      throw new Error(`Screenshot source symlink: ${relative}`);
  }
  return absolute;
}
function tailwindInputs(root) {
  const found = [];
  function visit(relative) {
    const directory = safePath(root, relative);
    if (!existsSync(directory)) return;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const name = `${relative}/${entry.name}`;
      if (entry.isSymbolicLink())
        throw new Error(`Screenshot source symlink: ${name}`);
      if (entry.isDirectory()) visit(name);
      else if (entry.isFile() && /\.(js|ts|jsx|tsx)$/.test(entry.name))
        found.push(name);
    }
  }
  visit("src");
  return found;
}
function git(root, args) {
  return execFileSync("git", args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
}
function digest(files) {
  const hash = createHash("sha256");
  for (const file of files)
    hash.update(file.path).update("\0").update(file.sha256).update("\n");
  return hash.digest("hex");
}
export function createScreenshotSource(
  projectRoot,
  modulePaths,
  { strict = false } = {},
) {
  const root = path.resolve(projectRoot);
  if (
    !Array.isArray(modulePaths) ||
    modulePaths.some((p) => typeof p !== "string")
  )
    throw new Error("Invalid module input list");
  const modules = [...new Set(modulePaths)].sort();
  const paths = [
    ...new Set([
      ...modules,
      ...tailwindInputs(root),
      ...fixed.filter((p) => existsSync(safePath(root, p))),
    ]),
  ].sort();
  for (const p of paths)
    if (!lstatSync(safePath(root, p)).isFile())
      throw new Error(`Missing screenshot source: ${p}`);
  const tracked = new Set(
    git(root, ["ls-files", "-z", "--", ...paths])
      .split("\0")
      .filter(Boolean),
  );
  const untracked = paths.filter((p) => !tracked.has(p));
  const modified = git(root, [
    "diff",
    "--name-only",
    "-z",
    "HEAD",
    "--",
    ...paths,
  ])
    .split("\0")
    .filter(Boolean);
  const dirtyPaths = [...new Set([...untracked, ...modified])].sort();
  if (strict && dirtyPaths.length)
    throw new Error(
      `Screenshot inputs dirty or untracked: ${dirtyPaths.join(", ")}`,
    );
  const files = paths.map((p) => ({
    path: p,
    sha256: createHash("sha256")
      .update(readFileSync(safePath(root, p)))
      .digest("hex"),
  }));
  return {
    head: git(root, ["rev-parse", "HEAD"]).trim(),
    dirty: dirtyPaths.length > 0,
    dirtyPaths,
    strict,
    modules,
    files,
    digest: digest(files),
  };
}
export function assertScreenshotSource(root, built, { strict = false } = {}) {
  if (
    !built ||
    !Array.isArray(built.modules) ||
    typeof built.digest !== "string"
  )
    throw new Error("Invalid screenshot source evidence");
  const current = createScreenshotSource(root, built.modules, { strict });
  if (current.digest !== built.digest)
    throw new Error(
      "Screenshot source digest mismatch; rebuild before capture",
    );
  if (strict && (built.dirty || current.head !== built.head))
    throw new Error(
      "Strict screenshot source mismatch; rebuild from the current clean checkout",
    );
  return {
    mode: strict ? "strict" : "preview",
    exactCommit: strict,
    builtHead: built.head,
    currentHead: current.head,
    builtDirty: built.dirty,
    currentDirty: current.dirty,
    digest: current.digest,
  };
}
