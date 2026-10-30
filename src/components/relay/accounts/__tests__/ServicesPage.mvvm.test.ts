import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("ServicesPage detail View dependency boundary", () => {
  it("leaves account queries, activity and usage commands to its original-owner VM", () => {
    const source = ["ServicesPage.tsx", "ServiceAccountDetailsView.tsx"]
      .map((name) =>
        readFileSync(`src/components/relay/accounts/${name}`, "utf8"),
      )
      .join("\n");
    const forbidden = [
      "useServiceAccounts",
      "useQuery",
      "useQueryClient",
      "useRowBusy",
      "usageApi",
      "configuredApps",
    ];
    const imports = forbidden.filter((name) =>
      new RegExp(`\\b${name}\\b`).test(source),
    );
    expect(imports).toEqual([]);
  });
});
