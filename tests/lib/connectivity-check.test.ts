import { describe, expect, it } from "vitest";
import { parseModelProbeVerdict } from "@/lib/api/connectivity-check";

describe("parseModelProbeVerdict", () => {
  it("parses structured model lists", () => {
    expect(
      parseModelProbeVerdict(
        '{"kind":"models","total":4,"head":["alpha","beta","…"]}',
      ),
    ).toEqual({
      kind: "models",
      total: 4,
      head: ["alpha", "beta", "…"],
    });
  });

  it("parses image-only and HTTP verdicts", () => {
    expect(
      parseModelProbeVerdict('{"kind":"imageOnly","models":["gpt-image-1"]}'),
    ).toEqual({ kind: "imageOnly", models: ["gpt-image-1"] });
    expect(
      parseModelProbeVerdict('{"kind":"keyExpired","status":401}'),
    ).toEqual({ kind: "keyExpired", status: 401 });
  });

  it("returns null for malformed or unknown payloads", () => {
    expect(parseModelProbeVerdict("legacy Chinese summary")).toBeNull();
    expect(parseModelProbeVerdict('{"kind":"futureVerdict"}')).toBeNull();
    expect(parseModelProbeVerdict('{"kind":"models","total":"4"}')).toBeNull();
    expect(parseModelProbeVerdict(undefined)).toBeNull();
  });
});
