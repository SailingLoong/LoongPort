import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRef } from "react";
import { expect, it, vi } from "vitest";
import {
  ManagedAccountApplicationPanel,
  type ManagedAccountApplicationHandle,
} from "@/components/relay/accounts/ManagedAccountApplicationPanel";
const api = vi.hoisted(() => ({
  providers: vi.fn().mockResolvedValue({}),
  routing: vi.fn().mockResolvedValue({
    modeState: {
      status: "ready",
      mode: "direct",
      attached: false,
      currentProviderId: null,
      directProviderId: null,
      canWrite: true,
    },
  }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options: { defaultValue: string }) =>
      options.defaultValue,
  }),
}));
vi.mock("@/lib/api/minimaxCode", () => ({
  minimaxCodeApi: {
    list: vi.fn().mockResolvedValue([
      {
        id: "synthetic-mini",
        label: "Synthetic MiniMax",
        authorized: true,
        status: "ready",
      },
    ]),
  },
}));
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: {
    list: vi.fn().mockResolvedValue([]),
    listInferenceAccounts: vi.fn().mockResolvedValue([]),
  },
}));
vi.mock("@/lib/api/zcodeAccounts", () => ({
  zcodeAccountsApi: {
    library: vi.fn().mockResolvedValue({ profiles: [], pending: false }),
  },
}));
vi.mock("@/lib/api/traeAccounts", () => ({
  traeAccountsApi: { list: vi.fn().mockResolvedValue([]) },
}));
vi.mock("@/lib/api/providers", () => ({
  providersApi: { getAll: api.providers },
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { get: api.routing },
}));
it("locks reads to the current app, retains same-app drafts, and reports them to the original navigation guard", async () => {
  const client = new QueryClient();
  const state = createRef<ManagedAccountApplicationHandle>();
  const view = (appId: "claude" | "codex") => (
    <QueryClientProvider client={client}>
      <ManagedAccountApplicationPanel key={appId} appId={appId} ref={state} />
    </QueryClientProvider>
  );
  const rendered = render(view("codex"));
  await screen.findByRole("option", { name: /Synthetic MiniMax/ });
  expect(screen.getByLabelText("目标应用")).toHaveValue("codex");
  expect(screen.getByLabelText("目标应用")).toBeDisabled();
  expect(screen.getByLabelText("首次启用的应用")).toHaveValue("codex");
  expect(screen.getByLabelText("首次启用的应用")).toBeDisabled();
  expect(api.providers).toHaveBeenCalledWith("codex");
  expect(api.routing).toHaveBeenCalledWith("codex");
  fireEvent.change(screen.getByLabelText("官方模型 ID"), {
    target: { value: "synthetic-model" },
  });
  await waitFor(() => expect(state.current?.hasPendingChanges).toBe(true));
  rendered.rerender(view("codex"));
  expect(screen.getByLabelText("官方模型 ID")).toHaveValue("synthetic-model");
  // The shell owns the leave-draft confirmation; only its approved navigation remounts this view.
  rendered.rerender(view("claude"));
  await screen.findByRole("option", { name: /Synthetic MiniMax/ });
  expect(screen.getByLabelText("目标应用")).toHaveValue("claude");
  expect(screen.getByLabelText("官方模型 ID")).toHaveValue("");
  expect(screen.getByLabelText("账号")).toHaveValue("");
  expect(api.providers).toHaveBeenLastCalledWith("claude");
  expect(state.current?.hasPendingChanges).toBe(false);
});
