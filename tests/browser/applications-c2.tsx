import { createRoot } from "react-dom/client";
import {
  installApplicationsC2Fixture,
  type ApplicationsC2Scenario,
} from "./applications-c2-fixture";
import "@/index.css";

const scenario =
  new URLSearchParams(location.search).get("scenario") ?? "normal";
if (!["normal", "unknown", "pending"].includes(scenario))
  throw new Error("Unsupported C2 fixture scenario");
const fixture = installApplicationsC2Fixture(
  scenario as ApplicationsC2Scenario,
);
// Install official native mocks and language preference before importing App.
const { ApplicationsC2App, createApplicationsC2Client } =
  await import("./applications-c2-app");
const client = createApplicationsC2Client();
Object.assign(window, {
  applicationsC2: { ...fixture, settled: () => client.isFetching() === 0 },
});
createRoot(document.getElementById("root")!).render(
  <ApplicationsC2App client={client} />,
);
