import { Suspense } from "react";
import {
  render,
  screen,
  waitFor,
  fireEvent,
  act,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import App from "@/App";
import { emitTauriEvent } from "../msw/tauriMocks";
import { UNIVERSAL_PROVIDER_SYNCED } from "@/lib/api/events";
import { http, HttpResponse } from "msw";
import { server } from "../msw/server";
import { ThemeProvider } from "@/components/theme-provider";
import { providersApi } from "@/lib/api";
import { proxyApi } from "@/lib/api/proxy";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { settingsApi } from "@/lib/api";
import { relayApi } from "@/lib/api/relay";
import {
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
} from "@/config/constants";
vi.mock("@/components/UpdateBadge", () => ({ UpdateBadge: () => null }));
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
it("keeps the original shell navigable without credential queries or unlock controls", async () => {
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })),
  );
  server.use(
    http.post("http://tauri.local/set_window_theme", () =>
      HttpResponse.json(null),
    ),
  );
  localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
  const reads = [
    vi.spyOn(providersApi, "getAll"),
    vi.spyOn(providersApi, "updateTrayMenu"),
    vi.spyOn(proxyApi, "getProxyStatus"),
    vi.spyOn(proxyApi, "getProxyTakeoverStatus"),
    vi.spyOn(relayApi, "checkSession"),
    vi.spyOn(serviceOnboardingApi, "status"),
    vi.spyOn(settingsApi, "save"),
  ];
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["service-onboarding"], { shouldPrompt: true });
  client.setQueryData(["runtimeAvailability"], {
    credentialRuntimeAvailable: false,
  });
  render(
    <QueryClientProvider client={client}>
      <ThemeProvider>
        <Suspense fallback={null}>
          <App />
        </Suspense>
      </ThemeProvider>
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(screen.getByText("secrets.runtimeUnavailable")).toBeInTheDocument(),
  );
  expect(
    document.querySelector('[data-client-shell="true"]'),
  ).toBeInTheDocument();
  for (const read of reads) expect(read).not.toHaveBeenCalled();
  expect(screen.queryByText("provider.noProviders")).not.toBeInTheDocument();
  expect(screen.queryByText("secrets.unlockTitle")).not.toBeInTheDocument();
  expect(screen.queryByLabelText("secrets.password")).not.toBeInTheDocument();
  expect(
    screen.queryByText("secrets.retrySystemUnlock"),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "client.services" }));
  expect(screen.getByText("secrets.runtimeUnavailable")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "client.usage" }));
  expect(screen.getByText("secrets.runtimeUnavailable")).toBeInTheDocument();
  const settings = screen.getByRole("button", { name: "common.settings" });
  fireEvent.click(settings);
  expect(await screen.findByText("settings.theme")).toBeInTheDocument();
  expect(screen.queryByText("secrets.protectionTitle")).not.toBeInTheDocument();
  await act(async () => {
    emitTauriEvent(UNIVERSAL_PROVIDER_SYNCED, {});
  });
  for (const read of reads) expect(read).not.toHaveBeenCalled();
});

it("removes a previously visible environment-write banner when runtime becomes unavailable", async () => {
  localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
  sessionStorage.removeItem("env_banner_dismissed");
  const env = await import("@/lib/api/env");
  const conflict = {
    varName: "SYNTHETIC_CONFIG",
    varValue: "nonsecret-fixture",
    sourceType: "file" as const,
    sourcePath: "synthetic-config-only",
  };
  vi.spyOn(env, "checkAllEnvConflicts").mockResolvedValue({
    claude: [conflict],
  });
  vi.spyOn(env, "checkEnvConflicts").mockResolvedValue([]);
  const remove = vi.spyOn(env, "deleteEnvVars");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["runtimeAvailability"], {
    credentialRuntimeAvailable: true,
  });
  render(
    <QueryClientProvider client={client}>
      <Suspense fallback={null}>
        <App />
      </Suspense>
    </QueryClientProvider>,
  );
  expect(await screen.findByText("env.warning.title")).toBeInTheDocument();
  await act(async () => {
    client.setQueryData(["runtimeAvailability"], {
      credentialRuntimeAvailable: false,
    });
  });
  await screen.findByText("secrets.runtimeUnavailable");
  expect(screen.queryByText("env.warning.title")).not.toBeInTheDocument();
  expect(remove).not.toHaveBeenCalled();
});

it.each(["not-needed", "read-failed"])(
  "does not open onboarding from stale cached facts when runtime returns (%s)",
  async (outcome) => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
    localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
    let resolve!: (value: {
      shouldPrompt: boolean;
      completed: boolean;
      plazaVisible: boolean;
    }) => void;
    let reject!: (error: Error) => void;
    const pending = new Promise<{
      shouldPrompt: boolean;
      completed: boolean;
      plazaVisible: boolean;
    }>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    vi.spyOn(serviceOnboardingApi, "status").mockReturnValue(pending);
    const complete = vi.spyOn(serviceOnboardingApi, "complete");
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    client.setQueryDefaults(["runtimeAvailability"], { staleTime: Infinity });
    client.setQueryData(["runtimeAvailability"], {
      credentialRuntimeAvailable: false,
    });
    client.setQueryData(["service-onboarding"], {
      shouldPrompt: true,
      completed: false,
      plazaVisible: false,
    });
    render(
      <QueryClientProvider client={client}>
        <Suspense fallback={null}>
          <App />
        </Suspense>
      </QueryClientProvider>,
    );
    await screen.findByText("secrets.runtimeUnavailable");
    await act(async () => {
      client.setQueryData(["runtimeAvailability"], {
        credentialRuntimeAvailable: true,
      });
    });
    await waitFor(() => expect(serviceOnboardingApi.status).toHaveBeenCalled());
    expect
      .soft(document.querySelector("header[hidden]"))
      .not.toBeInTheDocument();
    await act(async () => {
      if (outcome === "read-failed") reject(new Error("status unavailable"));
      else
        resolve({ shouldPrompt: false, completed: true, plazaVisible: false });
    });
    await waitFor(() =>
      expect(client.getQueryState(["service-onboarding"])?.fetchStatus).toBe(
        "idle",
      ),
    );
    expect(document.querySelector("header[hidden]")).not.toBeInTheDocument();
    expect(complete).not.toHaveBeenCalled();
  },
);

it("fails closed locally when the runtime availability read itself fails", async () => {
  localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
  server.use(
    http.post(
      "http://tauri.local/get_runtime_availability",
      () => new HttpResponse("runtime unavailable", { status: 500 }),
    ),
  );
  const read = vi.spyOn(providersApi, "getAll");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Suspense fallback={null}>
        <App />
      </Suspense>
    </QueryClientProvider>,
  );
  await screen.findByText("secrets.runtimeUnavailable");
  expect(client.getQueryState(["runtimeAvailability"])?.status).toBe("error");
  expect(read).not.toHaveBeenCalled();
  expect(
    document.querySelector('[data-client-shell="true"]'),
  ).toBeInTheDocument();
  expect(screen.queryByLabelText("secrets.password")).not.toBeInTheDocument();
});

it("offers local same-key recovery in the existing shell and waits for actual runtime readiness", async () => {
  localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
  const protection = await import("@/lib/api/credentialProtection");
  const locked = {
    mode: "encrypted" as const,
    status: "key_required" as const,
    shouldPrompt: false,
    canConfigure: false,
    canUnlock: true,
    canSwitchToPlaintext: false,
    canCreateSeparateConfiguration: false,
    reviewToken: "synthetic-local-recovery",
    restartRequired: false,
    errorCode: null,
  };
  vi.spyOn(protection.credentialProtectionApi, "status").mockResolvedValue(
    locked,
  );
  const unlock = vi
    .spyOn(protection.credentialProtectionApi, "unlock")
    .mockResolvedValue({
      ...locked,
      status: "available",
      canUnlock: false,
    });
  // A successful recovery response is not itself the runtime readiness authority.
  server.use(
    http.post("http://tauri.local/get_runtime_availability", () =>
      HttpResponse.json({ credentialRuntimeAvailable: false }),
    ),
  );
  const read = vi.spyOn(providersApi, "getAll");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["runtimeAvailability"], {
    credentialRuntimeAvailable: false,
  });
  render(
    <QueryClientProvider client={client}>
      <Suspense fallback={null}>
        <App />
      </Suspense>
    </QueryClientProvider>,
  );
  const input = await screen.findByLabelText("storageProtection.keyLabel");
  expect(
    document.querySelector('[data-client-shell="true"]'),
  ).toBeInTheDocument();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  fireEvent.change(input, { target: { value: "  原密钥🔑  " } });
  fireEvent.click(
    screen.getByRole("button", { name: "storageProtection.unlock" }),
  );
  await waitFor(() =>
    expect(unlock).toHaveBeenCalledWith(
      "  原密钥🔑  ",
      "synthetic-local-recovery",
    ),
  );
  await screen.findByText("storageProtection.saved");
  expect(read).not.toHaveBeenCalled();
  expect(screen.getByText("secrets.runtimeUnavailable")).toBeVisible();
});
