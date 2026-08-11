import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const MAIN = readFileSync(resolve(__dirname, "../../src/main.tsx"), "utf8");

describe("frontend diagnostics bootstrap", () => {
  it("installs the console bridge before global error handlers", () => {
    const bridge = MAIN.indexOf("installConsoleLogBridge();");
    const handlers = MAIN.indexOf("installGlobalErrorHandlers();");

    expect(bridge).toBeGreaterThanOrEqual(0);
    expect(handlers).toBeGreaterThan(bridge);
  });
});
