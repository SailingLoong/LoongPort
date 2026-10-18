import { describe, expect, it } from "vitest";
import {
  createCatalogRow,
  catalogRowsMatchModels,
} from "@/components/providers/forms/CodexFormFields";

describe("original catalog row capability fidelity", () => {
  it("retains explicit empty levels and mode when another visible field is edited", () => {
    const row = createCatalogRow({
      model: "alias",
      reasoningLevels: [],
      reasoningMode: "unknown",
    } as any);
    expect(row).toMatchObject({
      model: "alias",
      reasoningLevels: [],
      reasoningMode: "unknown",
    });
  });
  it("distinguishes missing levels from an explicit empty declaration", () => {
    expect(
      catalogRowsMatchModels(
        [{ model: "alias" }],
        [{ model: "alias", reasoningLevels: [] }],
      ),
    ).toBe(false);
  });
  it("rebuilds rows when only the explicit reasoning mode changes", () => {
    expect(
      catalogRowsMatchModels(
        [{ model: "alias", reasoningMode: "toggle" }] as any,
        [{ model: "alias", reasoningMode: "required" }] as any,
      ),
    ).toBe(false);
  });
});
