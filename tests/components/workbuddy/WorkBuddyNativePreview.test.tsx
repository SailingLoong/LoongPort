import { beforeEach, expect, it, vi } from "vitest";
import { StrictMode } from "react";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkBuddyNativePreview } from "@/components/workbuddy/WorkBuddyNativePreview";
const api = vi.hoisted(() => ({ previewNative: vi.fn() }));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const valid = {
  sourceVerification: "knownLocationOnly",
  authFile: "recognizedPersonalFormat",
  realm: "www.workbuddy.cn",
  logoutMarkerPresent: false,
  activationAvailable: false,
};
const inspect = () =>
  screen.getByRole("button", { name: "workbuddy.nativePreview.inspect" });
beforeEach(() => {
  vi.resetAllMocks();
  api.previewNative.mockResolvedValue(valid);
});
it("StrictMode mount never reads; explicit click renders only observed format", async () => {
  render(
    <StrictMode>
      <WorkBuddyNativePreview />
    </StrictMode>,
  );
  expect(api.previewNative).not.toHaveBeenCalled();
  await userEvent.click(inspect());
  expect(
    await screen.findByText("workbuddy.nativePreview.recognized"),
  ).toBeVisible();
  expect(
    screen.getByText("workbuddy.nativePreview.realmWorkbuddy"),
  ).toBeVisible();
  expect(
    screen.getByText("workbuddy.nativePreview.notActivation"),
  ).toBeVisible();
  expect(api.previewNative.mock.calls).toEqual([[]]);
});
it("double click remains single flight and late completion cannot update a remount", async () => {
  let resolve!: (v: unknown) => void;
  api.previewNative.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const first = render(<WorkBuddyNativePreview />);
  await userEvent.dblClick(inspect());
  expect(api.previewNative).toHaveBeenCalledTimes(1);
  first.unmount();
  render(<WorkBuddyNativePreview />);
  await act(async () => resolve(valid));
  expect(
    screen.queryByText("workbuddy.nativePreview.recognized"),
  ).not.toBeInTheDocument();
});
it.each([
  [
    { ...valid, realm: "www.codebuddy.cn", logoutMarkerPresent: true },
    ["recognized", "realmCodebuddy", "markerPresent"],
  ],
  [{ ...valid, authFile: "missing", realm: null }, ["missing", "markerAbsent"]],
])(
  "renders fixed realm/presence wording for supported DTO %#",
  async (value, labels) => {
    api.previewNative.mockResolvedValue(value);
    render(<WorkBuddyNativePreview />);
    await userEvent.click(inspect());
    for (const label of labels)
      expect(
        await screen.findByText(`workbuddy.nativePreview.${label}`),
      ).toBeVisible();
  },
);
it.each([
  "unsafeSource",
  "sourceChanged",
  "unreadableSource",
  "invalidDocument",
  "unsupportedContext",
  "unsupportedPlatform",
])("maps fixed error %s without data", async (error) => {
  api.previewNative.mockRejectedValue(error);
  render(<WorkBuddyNativePreview />);
  await userEvent.click(inspect());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    `workbuddy.nativePreview.error.${error}`,
  );
});
it.each([
  null,
  { ...valid, realm: "secret-token" },
  { ...valid, activationAvailable: true },
  { ...valid, sourceVerification: "verifiedInstall" },
  { ...valid, logoutMarkerPresent: "secret-token" },
  { ...valid, authFile: "missing" },
  { ...valid, authFile: "secret-token" },
])(
  "rejects unknown or inconsistent payload without rendering values %#",
  async (value) => {
    api.previewNative.mockResolvedValue(value);
    render(<WorkBuddyNativePreview />);
    await userEvent.click(inspect());
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "workbuddy.nativePreview.error.unknown",
    );
    expect(document.body).not.toHaveTextContent("secret-token");
    expect(
      screen.queryByText("workbuddy.nativePreview.recognized"),
    ).not.toBeInTheDocument();
  },
);
it("unknown rejection is not rendered and a later failure clears stale success", async () => {
  render(<WorkBuddyNativePreview />);
  await userEvent.click(inspect());
  expect(
    await screen.findByText("workbuddy.nativePreview.recognized"),
  ).toBeVisible();
  api.previewNative.mockRejectedValue(new Error("secret-token /private/path"));
  await userEvent.click(inspect());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.nativePreview.error.unknown",
  );
  expect(document.body).not.toHaveTextContent("secret-token");
  expect(
    screen.queryByText("workbuddy.nativePreview.recognized"),
  ).not.toBeInTheDocument();
});

it("does not retain or display unexpected fields in an otherwise recognized reply", async () => {
  api.previewNative.mockResolvedValue({
    ...valid,
    token: "secret-token",
    nickname: "/private/path",
  });
  render(<WorkBuddyNativePreview />);
  await userEvent.click(inspect());
  expect(
    await screen.findByText("workbuddy.nativePreview.recognized"),
  ).toBeVisible();
  expect(document.body).not.toHaveTextContent("secret-token");
  expect(document.body).not.toHaveTextContent("/private/path");
});
