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
  return lines.slice(start + 1, end < 0 ? undefined : end).join("\n");
}

describe("CI manual verification entry point", () => {
  it("pins the backend Node subprocess runtime before running Rust tests", () => {
    const backend = job("backend-tests");
    const setup = backend.indexOf("uses: actions/setup-node@v6");
    expect(setup).toBeGreaterThanOrEqual(0);
    expect(backend.slice(setup)).toMatch(/node-version-file: \.node-version/);
    expect(setup).toBeLessThan(backend.indexOf("- name: Run tests"));
    expect(backend).not.toMatch(/continue-on-error|--no-fail-fast/);
  });

  it("checks the real pinned Node pipe and exports its exact executable", () => {
    const backend = job("backend-tests");
    const script = backend.match(
      /node --input-type=module <<'NODE'\n([\s\S]+?)\n          NODE/,
    );
    expect(
      script,
      "backend Node preflight must execute before cargo",
    ).not.toBeNull();
    const directory = mkdtempSync(resolve(tmpdir(), "backend-node-preflight-"));
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
  });

  it("does not add a Node runtime to compile-only or WSL filesystem jobs", () => {
    expect(job("backend")).not.toContain("uses: actions/setup-node");
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

  it.each(["frontend", "backend", "backend-windows-wsl2"])(
    "forces %s on manual dispatch while keeping PR path filtering",
    (name) => {
      expect(job(name)).toMatch(
        /if: github\.event_name != 'pull_request' \|\| needs\.changes\.outputs\.(frontend|backend) == 'true'/,
      );
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
        .match(/needs: \[([^\]]+)\]/)![1]
        .split(",")
        .map((name) => name.trim());
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
