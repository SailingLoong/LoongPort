import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";

it("keeps setup transaction and page decisions in the consumed view model", () => {
  const view = readFileSync(
    resolve(
      process.cwd(),
      "src/components/relay/onboarding/RelaySetupConfiguration.tsx",
    ),
    "utf8",
  );
  expect(view).toContain("useRelaySetupConfigurationViewModel");
  expect(view).not.toMatch(
    /serviceOnboardingApi|useQueryClient|useRelaySetup\(|surfaceRevision|finishing\.current|setRetryApps|setup\.receipt/,
  );
});
