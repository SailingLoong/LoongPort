import { fireEvent, render, screen } from "@testing-library/react";
import { vi } from "vitest";

import { ProfileSwitcher } from "@/components/profiles/ProfileSwitcher";
import type { ProfilesResponse } from "@/lib/api/profiles";

const useProfilesQuery = vi.fn();

Element.prototype.scrollIntoView = vi.fn();

vi.mock("@/lib/query/profiles", () => ({
  useProfilesQuery: () => useProfilesQuery(),
  useApplyProfileMutation: () => ({ mutate: vi.fn() }),
  useClearProfileMutation: () => ({ mutate: vi.fn() }),
  useCreateProfileMutation: () => ({ mutate: vi.fn(), isPending: false }),
}));

vi.mock("@/components/profiles/ProfileManageDialog", () => ({
  ProfileManageDialog: () => null,
}));

const emptyPayload = {
  providers: { claude: null, "claude-desktop": null, codex: null },
  mcp: { claude: null, "claude-desktop": null, codex: null },
  skills: { claude: null, "claude-desktop": null, codex: null },
  prompts: { claude: null, "claude-desktop": null, codex: null },
};

describe("ProfileSwitcher backend-owned app facts", () => {
  it("renders support and scope exactly as returned by the backend", () => {
    useProfilesQuery.mockReturnValue({
      data: {
        profiles: [],
        apps: [
          {
            app: "codex-image",
            supported: true,
            scope: "codex",
            currentProfileId: null,
          },
        ],
      } satisfies ProfilesResponse,
    });

    render(<ProfileSwitcher activeApp="codex-image" />);

    expect(screen.getByRole("combobox")).toBeInTheDocument();
  });

  it("hides apps the backend marks unsupported", () => {
    useProfilesQuery.mockReturnValue({
      data: {
        profiles: [],
        apps: [
          {
            app: "codex",
            supported: false,
            scope: "codex",
            currentProfileId: null,
          },
        ],
      } satisfies ProfilesResponse,
    });

    render(<ProfileSwitcher activeApp="codex" />);

    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("renders backend snapshot presence instead of inspecting payload slots", () => {
    useProfilesQuery.mockReturnValue({
      data: {
        profiles: [
          {
            id: "profile-1",
            name: "Project One",
            payload: {
              ...emptyPayload,
              providers: {
                ...emptyPayload.providers,
                claude: "provider-1",
              },
            },
            scopeSnapshots: [
              { scope: "claude", hasSnapshot: false },
              { scope: "claude-desktop", hasSnapshot: false },
              { scope: "codex", hasSnapshot: false },
            ],
          },
        ],
        apps: [
          {
            app: "claude",
            supported: true,
            scope: "claude",
            currentProfileId: null,
          },
        ],
      } satisfies ProfilesResponse,
    });

    render(<ProfileSwitcher activeApp="claude" />);
    fireEvent.click(screen.getByRole("combobox"));

    expect(screen.getByText("profiles.noSnapshotForScope")).toBeInTheDocument();
  });
});
