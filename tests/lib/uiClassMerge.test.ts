import { cn } from "@/lib/utils";

it("keeps semantic typography and colors independent in either order", () => {
  expect(cn("text-action-fg", "text-body")).toBe("text-action-fg text-body");
  expect(cn("text-caption", "text-fg-2")).toBe("text-caption text-fg-2");
});

it("replaces semantic font sizes, corners and shadows within their own roles", () => {
  expect(cn("text-body", "text-caption")).toBe("text-caption");
  expect(cn("rounded-control", "rounded-panel")).toBe("rounded-panel");
  expect(cn("shadow-v7-sm", "shadow-v7-lg")).toBe("shadow-v7-lg");
});
