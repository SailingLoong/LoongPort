import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
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
