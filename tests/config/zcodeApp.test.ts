import { describe, expect, it } from "vitest";
import {
  APP_IDS,
  APP_DISPLAY_NAME,
  DEFAULT_VISIBLE_APPS,
  MCP_APP_IDS,
  PROXY_APP_IDS,
  SKILLS_APP_IDS,
  PROVIDER_STORE_APP_IDS,
} from "@/config/appConfig";

describe("ZCode native configuration entry", () => {
  it("provides an opt-in application entry without unsupported capabilities", () => {
    expect(APP_IDS).toContain("zcode");
    expect((APP_DISPLAY_NAME as Record<string, string>).zcode).toBe("ZCode");
    expect(
      (DEFAULT_VISIBLE_APPS as unknown as Record<string, boolean>).zcode,
    ).toBe(false);
    for (const ids of [
      MCP_APP_IDS,
      PROXY_APP_IDS,
      SKILLS_APP_IDS,
      PROVIDER_STORE_APP_IDS,
    ])
      expect(ids).not.toContain("zcode");
  });
});
