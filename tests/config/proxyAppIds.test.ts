import { describe, expect, it } from "vitest";
import { PROXY_APP_IDS } from "@/config/appConfig";

describe("local routing app list", () => {
  it("only exposes applications with local routing support", () => {
    expect(PROXY_APP_IDS).toEqual(["claude", "codex", "gemini", "grokbuild"]);
  });
});
