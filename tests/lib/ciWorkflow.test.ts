import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { describe, expect, it } from "vitest";

const workflow = readFileSync(
  resolve(__dirname, "../../.github/workflows/ci.yml"),
  "utf8",
);

function job(name: string) {
  const lines = workflow.split("\n");
  const start = lines.indexOf(`  ${name}:`);
  if (start < 0) throw new Error(`Missing CI job: ${name}`);
  const end = lines.findIndex(
    (line, index) => index > start && /^  [\w-]+:/.test(line),
  );
  let block = lines.slice(start + 1, end < 0 ? undefined : end).join("\n");
  // Resolve only the two CI whole-step-array aliases, not arbitrary YAML.
  const alias = block.match(/^    steps: \*([a-z_]+)$/m);
  if (alias) {
    if (!["backend_archive_steps", "backend_test_steps"].includes(alias[1])) {
      throw new Error(`Unexpected steps alias: ${alias[1]}`);
    }
    const anchorStart = lines.indexOf(`    steps: &${alias[1]}`);
    if (anchorStart < 0) throw new Error(`Missing steps anchor: ${alias[1]}`);
    const anchorEnd = lines.findIndex(
      (line, index) => index > anchorStart && /^  [\w-]+:/.test(line),
    );
    const steps = lines
      .slice(anchorStart + 1, anchorEnd < 0 ? undefined : anchorEnd)
      .join("\n");
    block = block.replace(alias[0], `    steps:\n${steps}`);
  }
  return block;
}

const backendTestJobs = [
  "backend-tests-linux",
  "backend-tests-windows",
  "backend-tests-macos",
];
const backendBuildJobs = [
  "backend-test-build-linux",
  "backend-test-build-windows",
  "backend-test-build-macos",
];

describe("CI manual verification entry point", () => {
  it.each(backendTestJobs)(
    "pins %s Node subprocess runtime before Rust tests",
    (name) => {
      const backend = job(name);
      const setup = backend.indexOf("uses: actions/setup-node@v6");
      expect(setup).toBeGreaterThanOrEqual(0);
      expect(backend.slice(setup)).toMatch(/node-version-file: \.node-version/);
      expect(setup).toBeLessThan(backend.indexOf("- name: Run tests"));
      expect(backend).not.toMatch(/continue-on-error|--no-fail-fast/);
    },
  );

  it.each(backendTestJobs)(
    "checks %s pinned Node pipe and exports its executable",
    (name) => {
      const backend = job(name);
      const script = backend.match(
        /node --input-type=module <<'NODE'\n([\s\S]+?)\n          NODE/,
      );
      expect(
        script,
        "backend Node preflight must execute before cargo",
      ).not.toBeNull();
      const directory = mkdtempSync(
        resolve(tmpdir(), "backend-node-preflight-"),
      );
      const environmentFile = resolve(directory, "github-env");
      try {
        const result = spawnSync(process.execPath, ["--input-type=module"], {
          cwd: resolve(__dirname, "../.."),
          input: script![1]
            .split("\n")
            .map((line) => line.slice(10))
            .join("\n"),
          env: { ...process.env, GITHUB_ENV: environmentFile },
          encoding: "utf8",
          timeout: 35_000,
        });
        expect(result.error).toBeUndefined();
        expect(result.status, result.stderr).toBe(0);
        expect(result.stdout).toContain(
          `Backend Node ${process.version}: ${process.execPath}`,
        );
        expect(result.stdout).toContain("Backend Node pipe preflight passed");
        expect(readFileSync(environmentFile, "utf8")).toBe(
          `QODER_HOST_NODE=${process.execPath}\n`,
        );
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    },
  );

  it("does not add a Node runtime to compile-only or WSL filesystem jobs", () => {
    for (const name of ["backend", ...backendBuildJobs]) {
      expect(job(name)).not.toContain("uses: actions/setup-node");
    }
    expect(job("backend-windows-wsl2")).not.toContain(
      "uses: actions/setup-node",
    );
  });

  it("runs backend tests when their pinned runtime or worker inputs change", () => {
    const backendPaths = job("changes").split("            backend:\n")[1];
    for (const path of [
      ".node-version",
      "scripts/qoder/**",
      "tests/qoder-runtime/**",
    ]) {
      expect(backendPaths).toContain(`- "${path}"`);
    }
  });

  it.each([
    ["pull_request", false],
    ["push", true],
    ["workflow_dispatch", true],
  ])("checks out the path-filter input for %s: %s", (event, expected) => {
    const condition = job("changes").match(
      /- name: Checkout\n\s+if: github\.event_name (==|!=) '([^']+)'/,
    );
    expect(condition).not.toBeNull();
    const shouldCheckout =
      condition![1] === "=="
        ? event === condition![2]
        : event !== condition![2];
    expect(shouldCheckout).toBe(expected);
  });

  it.each([
    "frontend",
    "backend",
    "backend-windows-wsl2",
    ...backendBuildJobs,
    ...backendTestJobs,
  ])("forces %s on manual dispatch while keeping PR path filtering", (name) => {
    expect(job(name)).toMatch(
      /if: github\.event_name != 'pull_request' \|\| needs\.changes\.outputs\.(frontend|backend) == 'true'/,
    );
  });

  it.each([
    ["linux", "ubuntu-22.04", 2],
    ["windows", "windows-latest", 4],
    ["macos", "macos-14", 2],
  ])(
    "builds %s once without blocking other platforms",
    (platform, os, count) => {
      const producer = job(`backend-test-build-${platform}`);
      const consumer = job(`backend-tests-${platform}`);
      expect(producer).toContain(`os: [${os}]`);
      expect(consumer).toContain(
        `needs: [changes, backend-test-build-${platform}]`,
      );
      expect(consumer).toContain(
        "name: Backend Tests (${{ matrix.os }} ${{ matrix.shard }}/${{ matrix.shards }})",
      );
      expect(consumer.match(/- \{ os:/g)).toHaveLength(Number(count));
      expect(producer).toContain("nextest_archive.py build");
      expect(consumer).toContain("nextest_archive.py run");
      expect(consumer).not.toContain("Swatinem/rust-cache");
      expect(consumer).not.toContain("cargo nextest run");
      for (const name of [
        `backend-test-build-${platform}`,
        `backend-tests-${platform}`,
      ]) {
        expect(job("required-checks")).toContain(name);
      }
    },
  );

  it.each([
    ["success", "skipped", 0],
    ["failure", "skipped", 1],
    ["cancelled", "skipped", 1],
    ["success", "failure", 1],
  ])(
    "requires path detection %s and runtime checks %s to produce exit %s",
    (changes, runtime, expected) => {
      const gate = job("required-checks");
      const dependencies = gate
        .match(/needs:\s*\[([^\]]+)\]/)![1]
        .split(",")
        .map((name) => name.trim())
        .filter(Boolean);
      const results = dependencies.map((name) =>
        name === "changes"
          ? changes
          : name === "workflow-lint"
            ? "success"
            : runtime,
      );
      const script = gate
        .split("        run: |\n")[1]
        .split("\n")
        .filter((line) => line.startsWith("          "))
        .map((line) => line.slice(10))
        .join("\n")
        .replace(
          /\$\{\{ join\(needs\.\*\.result, ' '\) \}\}/,
          results.join(" "),
        );
      expect(spawnSync("bash", ["-c", script]).status).toBe(expected);
    },
  );
});
