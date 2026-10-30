import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ZCodeClaimControls } from "@/components/zcode/ZCodeClaimControls";
import { zcodeClaimApi, type ClaimState } from "@/lib/api/zcodeClaim";
vi.mock("@/lib/api/zcodeClaim", () => ({
  zcodeClaimApi: {
    state: vi.fn(),
    setAuto: vi.fn(),
    start: vi.fn(),
    cancel: vi.fn(),
  },
}));
const state: ClaimState = {
  consentRevision: "context:0",
  enabled: false,
  participants: [],
  records: {},
  busy: false,
};
const profiles = [
  { id: "a", label: "Account A" },
  { id: "b", label: "Account B" },
];
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(zcodeClaimApi.state).mockResolvedValue(state);
  vi.mocked(zcodeClaimApi.start).mockResolvedValue(state);
  vi.mocked(zcodeClaimApi.setAuto).mockResolvedValue(state);
});
it("reads state without claiming; selection and auto participation require explicit actions", async () => {
  render(
    <ZCodeClaimControls dataRoot="/synthetic/library" profiles={profiles} />,
  );
  await screen.findAllByText("Unknown");
  expect(zcodeClaimApi.start).not.toHaveBeenCalled();
  expect(screen.getByLabelText("Automatic claims")).not.toBeChecked();
  fireEvent.click(screen.getByLabelText("Select Account A"));
  fireEvent.click(screen.getByRole("button", { name: "Check selected" }));
  await waitFor(() =>
    expect(zcodeClaimApi.start).toHaveBeenCalledWith(
      "/synthetic/library",
      ["a"],
      true,
    ),
  );
  fireEvent.click(screen.getByLabelText("Automatic claims"));
  expect(zcodeClaimApi.setAuto).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "Apply automatic participation" }),
  );
  await waitFor(() =>
    expect(zcodeClaimApi.setAuto).toHaveBeenCalledWith(
      "/synthetic/library",
      true,
      ["a"],
      "context:0",
    ),
  );
});
it("shows distinct results, safe error, plan expiry and backend eligibility", async () => {
  vi.mocked(zcodeClaimApi.state).mockResolvedValue({
    ...state,
    records: {
      a: {
        status: "verificationRequired",
        plans: [],
        planId: null,
        planName: null,
        startsAt: null,
        endsAt: null,
        checkedAt: null,
        reason: "secret backend message",
        canClaim: false,
      },
      b: {
        status: "claimable",
        plans: [],
        planId: "p",
        planName: "Daily plan",
        startsAt: null,
        endsAt: 2000000000,
        checkedAt: null,
        reason: null,
        canClaim: true,
      },
    },
  });
  render(
    <ZCodeClaimControls dataRoot="/synthetic/library" profiles={profiles} />,
  );
  await screen.findByText("User verification required");
  expect(screen.queryByText("secret backend message")).not.toBeInTheDocument();
  expect(screen.getByText(/Daily plan/)).toBeInTheDocument();
  expect(
    within(screen.getByRole("group", { name: "Account B" })).getByText(
      /Expires:/,
    ),
  ).toBeInTheDocument();
  expect(
    within(screen.getByRole("group", { name: "Account A" })).getByRole(
      "button",
      { name: "Claim" },
    ),
  ).toBeDisabled();
  fireEvent.click(
    within(screen.getByRole("group", { name: "Account B" })).getByRole(
      "button",
      { name: "Claim" },
    ),
  );
  await waitFor(() =>
    expect(zcodeClaimApi.start).toHaveBeenCalledWith(
      "/synthetic/library",
      ["b"],
      false,
    ),
  );
});

it.each([
  ["noClaim", "No eligible plan"],
  ["loginExpired", "Login expired"],
  ["resultPending", "Result pending"],
  ["cancelled", "Cancelled"],
] as const)(
  "renders %s independently without treating missing units as zero",
  async (status, text) => {
    vi.mocked(zcodeClaimApi.state).mockResolvedValue({
      ...state,
      records: {
        a: {
          status,
          plans: [
            {
              id: "plan",
              name: "Plan",
              description: null,
              priority: 1,
              units: null,
              grants: [],
            },
          ],
          planId: null,
          planName: null,
          startsAt: null,
          endsAt: Number.MAX_VALUE,
          checkedAt: null,
          reason: null,
          canClaim: false,
        },
      },
    });
    render(<ZCodeClaimControls profiles={profiles} />);
    await screen.findByText(text);
    expect(screen.getByText(/Units: Unknown/)).toBeInTheDocument();
    expect(screen.queryByText(/Units: 0/)).not.toBeInTheDocument();
  },
);

it("keeps backend claim results when an older polling read completes later", async () => {
  vi.useFakeTimers();
  try {
    const { act } = await import("@testing-library/react");
    let resolveRead!: (value: ClaimState) => void;
    const oldRead = new Promise<ClaimState>((resolve) => {
      resolveRead = resolve;
    });
    vi.mocked(zcodeClaimApi.state)
      .mockResolvedValueOnce(state)
      .mockReturnValueOnce(oldRead)
      .mockResolvedValue(state);
    render(<ZCodeClaimControls profiles={profiles} />);
    await act(async () => {});
    fireEvent.click(screen.getByLabelText("Select Account A"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    vi.mocked(zcodeClaimApi.start).mockResolvedValue({
      ...state,
      records: {
        a: {
          status: "resultPending",
          plans: [],
          planId: null,
          planName: null,
          startsAt: null,
          endsAt: null,
          checkedAt: null,
          reason: null,
          canClaim: false,
        },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Check selected" }));
    await act(async () => {});
    expect(screen.getByText("Result pending")).toBeInTheDocument();
    await act(async () => {
      resolveRead(state);
    });
    expect(screen.getByText("Result pending")).toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

it("ignores an older polling failure after a newer operation succeeds", async () => {
  vi.useFakeTimers();
  try {
    const { act } = await import("@testing-library/react");
    let rejectRead!: (reason: Error) => void;
    const oldRead = new Promise<ClaimState>((_, reject) => {
      rejectRead = reject;
    });
    vi.mocked(zcodeClaimApi.state)
      .mockResolvedValueOnce(state)
      .mockReturnValueOnce(oldRead);
    render(<ZCodeClaimControls profiles={profiles} />);
    await act(async () => {});
    fireEvent.click(screen.getByLabelText("Select Account A"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    fireEvent.click(screen.getByRole("button", { name: "Check selected" }));
    await act(async () => {});
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await act(async () => {
      rejectRead(new Error("synthetic stale read failure"));
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

it.each(["success", "failure"] as const)(
  "keeps the command result when a read started during it ends in %s",
  async (outcome) => {
    vi.useFakeTimers();
    try {
      const { act } = await import("@testing-library/react");
      let resolveOperation!: (value: ClaimState) => void;
      let resolveRead!: (value: ClaimState) => void;
      let rejectRead!: (reason: Error) => void;
      vi.mocked(zcodeClaimApi.start).mockReturnValueOnce(
        new Promise<ClaimState>((resolve) => {
          resolveOperation = resolve;
        }),
      );
      vi.mocked(zcodeClaimApi.state)
        .mockResolvedValueOnce(state)
        .mockReturnValueOnce(
          new Promise<ClaimState>((resolve, reject) => {
            resolveRead = resolve;
            rejectRead = reject;
          }),
        );
      render(<ZCodeClaimControls profiles={profiles} />);
      await act(async () => {});
      fireEvent.click(screen.getByLabelText("Select Account A"));
      fireEvent.click(screen.getByRole("button", { name: "Check selected" }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10000);
      });
      await act(async () => {
        resolveOperation({ ...state, busy: true });
      });
      expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
      await act(async () => {
        if (outcome === "success") resolveRead(state);
        else rejectRead(new Error("synthetic stale read failure"));
      });
      expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      expect(
        screen.queryByRole("button", { name: "Cancel" }),
      ).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Check selected" }),
      ).toBeEnabled();
    } finally {
      vi.useRealTimers();
    }
  },
);

it("keeps a command failure when a read started during it later succeeds", async () => {
  vi.useFakeTimers();
  try {
    const { act } = await import("@testing-library/react");
    let rejectOperation!: (reason: Error) => void;
    let resolveRead!: (value: ClaimState) => void;
    vi.mocked(zcodeClaimApi.start).mockReturnValueOnce(
      new Promise<ClaimState>((_, reject) => {
        rejectOperation = reject;
      }),
    );
    vi.mocked(zcodeClaimApi.state)
      .mockResolvedValueOnce(state)
      .mockReturnValueOnce(
        new Promise<ClaimState>((resolve) => {
          resolveRead = resolve;
        }),
      );
    render(<ZCodeClaimControls profiles={profiles} />);
    await act(async () => {});
    fireEvent.click(screen.getByLabelText("Select Account A"));
    fireEvent.click(screen.getByRole("button", { name: "Check selected" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    await act(async () => {
      rejectOperation(new Error("synthetic command failure"));
    });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => {
      resolveRead(state);
    });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

it("clears a polling failure when the pending command later succeeds", async () => {
  vi.useFakeTimers();
  try {
    const { act } = await import("@testing-library/react");
    let resolveOperation!: (value: ClaimState) => void;
    vi.mocked(zcodeClaimApi.start).mockReturnValueOnce(
      new Promise<ClaimState>((resolve) => {
        resolveOperation = resolve;
      }),
    );
    vi.mocked(zcodeClaimApi.state)
      .mockResolvedValueOnce(state)
      .mockRejectedValueOnce(new Error("synthetic polling failure"))
      .mockReturnValueOnce(new Promise<ClaimState>(() => {}));
    render(<ZCodeClaimControls profiles={profiles} />);
    await act(async () => {});
    fireEvent.click(screen.getByLabelText("Select Account A"));
    fireEvent.click(screen.getByRole("button", { name: "Check selected" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => {
      resolveOperation({ ...state, busy: true });
    });
    expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

it.each([
  ["openOfficialClient", "Open official ZCode, then check again"],
  ["notDue", "Plan is still valid; check after expiry"],
] as const)(
  "shows only the fixed safe instruction for %s",
  async (reason, message) => {
    vi.mocked(zcodeClaimApi.state).mockResolvedValue({
      ...state,
      records: {
        a: {
          status: "noClaim",
          plans: [],
          planId: null,
          planName: null,
          startsAt: null,
          endsAt: null,
          checkedAt: null,
          reason,
          canClaim: false,
        },
      },
    });
    render(<ZCodeClaimControls profiles={profiles} />);
    expect(await screen.findByText(message)).toBeInTheDocument();
  },
);

it("cancels using the currently selected library", async () => {
  vi.mocked(zcodeClaimApi.state).mockResolvedValue({ ...state, busy: true });
  vi.mocked(zcodeClaimApi.cancel).mockResolvedValue(state);
  render(
    <ZCodeClaimControls
      dataRoot="/synthetic/other-library"
      profiles={profiles}
    />,
  );
  fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
  await waitFor(() =>
    expect(zcodeClaimApi.cancel).toHaveBeenCalledWith(
      "/synthetic/other-library",
    ),
  );
});

it("keeps the reviewed consent revision when polling observes another window's revocation", async () => {
  vi.mocked(zcodeClaimApi.state).mockResolvedValueOnce({
    ...state,
    consentRevision: "context:1",
  } as ClaimState);
  render(
    <ZCodeClaimControls dataRoot="/synthetic/library" profiles={profiles} />,
  );
  await screen.findAllByText("Unknown");
  fireEvent.click(screen.getByLabelText("Select Account A"));
  fireEvent.click(screen.getByLabelText("Automatic claims"));
  vi.mocked(zcodeClaimApi.state).mockResolvedValue({
    ...state,
    consentRevision: "context:2",
  } as ClaimState);
  fireEvent(document, new Event("visibilitychange"));
  await waitFor(() => expect(zcodeClaimApi.state).toHaveBeenCalledTimes(2));
  fireEvent.click(
    screen.getByRole("button", { name: "Apply automatic participation" }),
  );
  await waitFor(() =>
    expect(zcodeClaimApi.setAuto).toHaveBeenCalledWith(
      "/synthetic/library",
      true,
      ["a"],
      "context:1",
    ),
  );
});

it("refreshes a rejected consent draft without retrying the grant", async () => {
  vi.mocked(zcodeClaimApi.state).mockResolvedValueOnce({
    ...state,
    consentRevision: "context:1",
  });
  render(
    <ZCodeClaimControls dataRoot="/synthetic/library" profiles={profiles} />,
  );
  await screen.findAllByText("Unknown");
  fireEvent.click(screen.getByLabelText("Select Account A"));
  fireEvent.click(screen.getByLabelText("Automatic claims"));
  vi.mocked(zcodeClaimApi.state).mockResolvedValue({
    ...state,
    consentRevision: "context:2",
  });
  vi.mocked(zcodeClaimApi.setAuto).mockRejectedValueOnce("changed");
  fireEvent.click(
    screen.getByRole("button", { name: "Apply automatic participation" }),
  );
  await waitFor(() =>
    expect(screen.getByLabelText("Automatic claims")).not.toBeChecked(),
  );
  expect(screen.getByLabelText("Select Account A")).not.toBeChecked();
  expect(zcodeClaimApi.setAuto).toHaveBeenCalledTimes(1);
});

it("headless commands enforce disabled admission before dispatch", async () => {
  const { renderHook, act } = await import("@testing-library/react");
  const { useZCodeClaimControls } =
    await import("@/components/zcode/viewmodels/useZCodeClaimControls");
  const { result } = renderHook(() =>
    useZCodeClaimControls({
      dataRoot: "/synthetic/library",
      profiles,
      disabled: true,
    }),
  );
  await waitFor(() => expect(result.current.loaded).toBe(true));
  await act(() => result.current.checkAccount("a"));
  expect(zcodeClaimApi.start).not.toHaveBeenCalled();
});
