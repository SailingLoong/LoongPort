import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { OrderProfilesMenu } from "../OrderProfilesMenu";

const notifications = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast: notifications }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { name?: string }) =>
      options?.name ? `${key} ${options.name}` : key,
  }),
}));

function setup() {
  const client = new QueryClient();
  const refresh = vi.spyOn(client, "invalidateQueries");
  const callbacks = {
    onLoadDraft: vi.fn(),
    onSaved: vi.fn(),
    onBusyChange: vi.fn(),
    onProfileRenamed: vi.fn(),
    onProfileRemoved: vi.fn(),
  };
  render(
    <QueryClientProvider client={client}>
      <OrderProfilesMenu
        appType="codex"
        state={{
          current: "Daily",
          profiles: [{ name: "Daily", providerIds: ["b", "removed", "a"] }],
        }}
        targetIds={["b", "a"]}
        storedIds={["a", "b", "c"]}
        {...callbacks}
      />
    </QueryClientProvider>,
  );
  return { ...callbacks, refresh };
}

async function openMenu() {
  await userEvent.click(screen.getByTitle("applications.orderProfiles"));
}
async function choose(action: string) {
  await openMenu();
  await userEvent.click(
    screen.getByRole("menuitem", { name: `applications.${action}` }),
  );
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(invoke).mockResolvedValue(undefined);
});

it("loads only known members in saved order without writing or appending other tiers", async () => {
  const { onLoadDraft, refresh } = setup();
  await openMenu();
  await userEvent.click(screen.getByRole("menuitem", { name: /^Daily/ }));
  expect(onLoadDraft).toHaveBeenCalledWith("Daily", ["b", "a"]);
  expect(invoke).not.toHaveBeenCalled();
  expect(refresh).not.toHaveBeenCalled();
});

it("keeps a failed save editable and saves the trimmed name with the exact candidate list on retry", async () => {
  const { onSaved, onLoadDraft, refresh } = setup();
  await choose("orderProfileSaveCurrent");
  const input = screen.getByRole("textbox");
  await userEvent.type(input, "   ");
  expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
  await userEvent.type(input, "Travel  ");
  vi.mocked(invoke).mockRejectedValueOnce(new Error("storage unavailable"));
  await userEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() => expect(notifications.error).toHaveBeenCalled());
  expect(screen.getByRole("dialog")).toBeVisible();
  expect(input).toHaveValue("   Travel  ");
  expect(onSaved).not.toHaveBeenCalled();
  expect(refresh).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(invoke).toHaveBeenLastCalledWith("save_order_profile", {
    appType: "codex",
    name: "Travel",
    providerIds: ["b", "a"],
  });
  expect(onSaved).toHaveBeenCalledWith("Travel", ["b", "a"]);
  expect(onLoadDraft).not.toHaveBeenCalled();
  expect(refresh).toHaveBeenCalledWith({
    queryKey: ["orderProfiles", "codex"],
  });
});

it("cancels naming without saving or replacing the active draft", async () => {
  const { onSaved } = setup();
  await choose("orderProfileSaveCurrent");
  await userEvent.type(screen.getByRole("textbox"), "Travel");
  await userEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(invoke).not.toHaveBeenCalled();
  expect(onSaved).not.toHaveBeenCalled();
});

it("preserves a rejected rename for retry without loading or changing the selected profile", async () => {
  const { onProfileRenamed, onLoadDraft } = setup();
  await openMenu();
  await userEvent.click(
    screen.getByRole("button", {
      name: "applications.orderProfileRename Daily",
    }),
  );
  expect(onLoadDraft).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "common.save" })).toBeDisabled();
  await userEvent.clear(screen.getByRole("textbox"));
  await userEvent.type(screen.getByRole("textbox"), " Travel ");
  vi.mocked(invoke).mockRejectedValueOnce(new Error("profile already exists"));
  await userEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() => expect(notifications.error).toHaveBeenCalled());
  expect(onProfileRenamed).not.toHaveBeenCalled();
  expect(screen.getByRole("textbox")).toHaveValue(" Travel ");
  await userEvent.click(screen.getByRole("button", { name: "common.save" }));
  await waitFor(() =>
    expect(onProfileRenamed).toHaveBeenCalledWith("Daily", "Travel"),
  );
  expect(invoke).toHaveBeenLastCalledWith("rename_order_profile", {
    appType: "codex",
    from: "Daily",
    to: "Travel",
  });
  expect(onLoadDraft).not.toHaveBeenCalled();
});

it("does not remove the draft on delete failure and notifies removal only after success", async () => {
  const { onProfileRemoved, onLoadDraft, refresh } = setup();
  await openMenu();
  vi.mocked(invoke).mockRejectedValueOnce(new Error("storage unavailable"));
  await userEvent.click(
    screen.getByRole("button", {
      name: "applications.orderProfileDelete Daily",
    }),
  );
  await waitFor(() => expect(notifications.error).toHaveBeenCalled());
  expect(onProfileRemoved).not.toHaveBeenCalled();
  expect(onLoadDraft).not.toHaveBeenCalled();
  expect(refresh).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", {
      name: "applications.orderProfileDelete Daily",
    }),
  );
  await waitFor(() => expect(onProfileRemoved).toHaveBeenCalledWith("Daily"));
  expect(invoke).toHaveBeenLastCalledWith("delete_order_profile", {
    appType: "codex",
    name: "Daily",
  });
  expect(refresh).toHaveBeenCalledTimes(1);
});

it.each(["Import", "Export"])(
  "treats cancelled %s file selection as no change",
  async (action) => {
    const { refresh, onLoadDraft, onSaved } = setup();
    vi.mocked(invoke).mockResolvedValue(null);
    await choose(`orderProfile${action}`);
    expect(invoke).toHaveBeenCalledWith(
      `${action.toLowerCase()}_order_profiles`,
      { appType: "codex" },
    );
    await waitFor(() =>
      expect(screen.getByTitle("applications.orderProfiles")).toBeEnabled(),
    );
    expect(notifications.success).not.toHaveBeenCalled();
    expect(notifications.error).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
    expect(onLoadDraft).not.toHaveBeenCalled();
    expect(onSaved).not.toHaveBeenCalled();
  },
);

it("reports an invalid import without replacing the draft, then refreshes profiles after a valid retry", async () => {
  const { refresh, onLoadDraft, onSaved } = setup();
  vi.mocked(invoke).mockRejectedValueOnce(new Error("invalid profile file"));
  await choose("orderProfileImport");
  await waitFor(() => expect(notifications.error).toHaveBeenCalled());
  expect(refresh).not.toHaveBeenCalled();
  expect(screen.getByTitle("applications.orderProfiles")).toHaveTextContent(
    "Daily",
  );
  vi.mocked(invoke).mockResolvedValueOnce(2);
  await choose("orderProfileImport");
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(1));
  expect(notifications.success).toHaveBeenCalledWith(
    "applications.orderProfilesImported",
  );
  expect(onLoadDraft).not.toHaveBeenCalled();
  expect(onSaved).not.toHaveBeenCalled();
});

it("reports export write failures and succeeds on retry without mutating the profile list", async () => {
  const { refresh, onLoadDraft, onSaved } = setup();
  vi.mocked(invoke).mockRejectedValueOnce(new Error("file is not writable"));
  await choose("orderProfileExport");
  await waitFor(() => expect(notifications.error).toHaveBeenCalled());
  vi.mocked(invoke).mockResolvedValueOnce("/tmp/profiles.json");
  await choose("orderProfileExport");
  await waitFor(() =>
    expect(notifications.success).toHaveBeenCalledWith(
      "applications.orderProfileExported",
    ),
  );
  expect(refresh).not.toHaveBeenCalled();
  expect(onLoadDraft).not.toHaveBeenCalled();
  expect(onSaved).not.toHaveBeenCalled();
});
