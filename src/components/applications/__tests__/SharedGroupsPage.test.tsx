import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { SharedGroupsPage } from "../SharedGroupsPage";
import {
  sharedRoutingApi,
  type SharedRoutingState,
} from "@/lib/api/sharedRouting";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
vi.mock("@/lib/api/sharedRouting", () => ({
  sharedRoutingApi: {
    get: vi.fn(),
    prepareGroup: vi.fn(),
    confirmGroup: vi.fn(),
    applyTarget: vi.fn(),
  },
}));
vi.mock("@/lib/api/serviceSources", () => ({
  serviceSourcesApi: { list: vi.fn() },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
let state: SharedRoutingState;
beforeEach(() => {
  vi.clearAllMocks();
  state = {
    appId: "codex",
    binding: null,
    groups: [
      {
        id: "daily",
        name: "Original group",
        revision: 4,
        members: [
          { sourceId: "missing-original", model: "model-first" },
          { sourceId: "known", model: "model-second" },
        ],
      },
    ],
    pendingRequest: null,
    canApply: true,
    blockedReason: null,
  };
  vi.mocked(sharedRoutingApi.get).mockImplementation(async () => state);
  vi.mocked(serviceSourcesApi.list).mockResolvedValue([
    {
      sourceId: "known",
      name: "Known source",
      account: { kind: "vendor", id: 1 },
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      bindings: [
        {
          appId: "codex",
          model: "model-second",
          providerId: "p",
          usageState: "enabled",
          canSelect: true,
        },
      ],
    },
  ]);
  vi.mocked(sharedRoutingApi.prepareGroup).mockImplementation(
    async (draft) => ({
      group: {
        id: draft.id ?? "native-new-id",
        name: draft.name,
        revision: (draft.expectedRevision ?? 0) + 1,
        members: draft.members,
      },
      expectedRevision: draft.expectedRevision,
      draftDigest: "native-digest",
      affectedAppIds: ["codex", "claude"],
    }),
  );
  vi.mocked(sharedRoutingApi.confirmGroup).mockImplementation(
    async (review) => {
      state = { ...state, groups: [review.group] };
      return review.group;
    },
  );
});
function mount() {
  const onOpenApplications = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <SharedGroupsPage appId="codex" onOpenApplications={onOpenApplications} />
    </QueryClientProvider>,
  );
  return { onOpenApplications };
}
it("retains unavailable identities and exact saved member order", async () => {
  mount();
  await screen.findByText("Original group");
  const rows = within(screen.getByTestId("group-members-daily")).getAllByRole(
    "listitem",
  );
  expect(rows[0]).toHaveTextContent("missing-original");
  expect(rows[1]).toHaveTextContent("Known source");
});
it("reorders a local group draft, shows native affected apps and confirms exactly once", async () => {
  const user = userEvent.setup();
  mount();
  await user.click(
    await screen.findByRole("button", { name: "sharedGroups.edit" }),
  );
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.moveDown 1" }),
  );
  expect(sharedRoutingApi.confirmGroup).not.toHaveBeenCalled();
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.preview" }),
  );
  await screen.findByText("sharedGroups.reviewTitle");
  expect(sharedRoutingApi.prepareGroup).toHaveBeenCalledWith(
    expect.objectContaining({
      id: "daily",
      expectedRevision: 4,
      members: [
        { sourceId: "known", model: "model-second" },
        { sourceId: "missing-original", model: "model-first" },
      ],
    }),
  );
  expect(screen.getByTestId("group-affected-apps")).toHaveTextContent(
    "apps.codex",
  );
  expect(screen.getByTestId("group-affected-apps")).toHaveTextContent(
    "apps.claude",
  );
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.confirm" }),
  );
  await waitFor(() =>
    expect(sharedRoutingApi.confirmGroup).toHaveBeenCalledTimes(1),
  );
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
});
it("creates a group without automatically binding the current app, then offers explicit application navigation", async () => {
  const user = userEvent.setup();
  const { onOpenApplications } = mount();
  await user.click(
    await screen.findByRole("button", { name: "sharedGroups.create" }),
  );
  await user.type(screen.getByLabelText("sharedGroups.name"), "New group");
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.addMember" }),
  );
  await user.selectOptions(
    screen.getByLabelText("sharedGroups.memberSource 1"),
    "known",
  );
  await user.selectOptions(
    screen.getByLabelText("sharedGroups.memberModel 1"),
    "model-second",
  );
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.preview" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "sharedGroups.confirm" }),
  );
  await screen.findByText("sharedGroups.savedNotBound");
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(onOpenApplications).not.toHaveBeenCalled();
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.openApplications" }),
  );
  expect(onOpenApplications).toHaveBeenCalledTimes(1);
});
it("checks an ambiguous group write without replaying it or claiming success", async () => {
  vi.mocked(sharedRoutingApi.confirmGroup).mockRejectedValue(
    new Error("synthetic lost reply"),
  );
  const user = userEvent.setup();
  mount();
  await user.click(
    await screen.findByRole("button", { name: "sharedGroups.edit" }),
  );
  await user.type(screen.getByLabelText("sharedGroups.name"), " edited");
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.preview" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "sharedGroups.confirm" }),
  );
  await screen.findByText("sharedGroups.outcomeUnknown");
  expect(sharedRoutingApi.confirmGroup).toHaveBeenCalledTimes(1);
  expect(
    screen.queryByText("sharedGroups.savedNotBound"),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "sharedGroups.preview" }),
  ).toBeDisabled();
  await user.click(
    screen.getByRole("button", { name: "sharedGroups.inspect" }),
  );
  expect(sharedRoutingApi.confirmGroup).toHaveBeenCalledTimes(1);
});
it("does not fabricate an empty group list after a read error", async () => {
  vi.mocked(sharedRoutingApi.get).mockRejectedValue(
    new Error("sensitive native error"),
  );
  mount();
  await screen.findByRole("alert");
  expect(screen.queryByText("sharedGroups.empty")).not.toBeInTheDocument();
  expect(screen.queryByText("sensitive native error")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "sharedGroups.create" }),
  ).toBeDisabled();
});
