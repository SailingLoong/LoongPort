import { expect, it, vi } from "vitest";
import { formatResetAt } from "../tierMetrics";
it("formats resets in the selected application language", () => {
  vi.spyOn(Date, "now").mockReturnValue(1_000_000);
  expect(formatResetAt(8200, "en")).toContain("2 hours");
  expect(formatResetAt(8200, "zh-CN")).toContain("2小时");
  vi.restoreAllMocks();
});
