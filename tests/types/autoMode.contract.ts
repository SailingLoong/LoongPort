import {
  autoModeApi,
  type EasyModeMode,
  type AutoModeStrategy,
} from "../../src/lib/api/autoMode";

// Historical responses remain readable. Do not narrow the wire decode types.
const historicalMode: EasyModeMode = "auto";
const historicalStrategy: AutoModeStrategy = "fastest";
void historicalMode;
void historicalStrategy;

autoModeApi.setEasyModeMode("claude", "manual");
autoModeApi.setStrategy("cheapest");
autoModeApi.setStrategy("known_expiry_first");
autoModeApi.setModel("claude", null);

// The backend has retired automatic policy mode and rejects this write.
// @ts-expect-error retired mode must not be advertised as a writable value
autoModeApi.setEasyModeMode("claude", "auto");
// @ts-expect-error speed policy is a read-only legacy value
autoModeApi.setStrategy("fastest");
