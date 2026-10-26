import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ZCodeDeviceManager } from "@/components/zcode/ZCodeDeviceManager";
import { zcodeDeviceApi, type ZCodeDeviceState } from "@/lib/api/zcodeDevices";
vi.mock("@/lib/api/zcodeDevices", () => ({
  zcodeDeviceApi: { state: vi.fn(), import: vi.fn(), select: vi.fn() },
}));
const id = "11111111-2222-4333-8444-555555555555";
const empty: ZCodeDeviceState = {
  devices: [],
  selectedDeviceMid: null,
  effectiveDeviceMid: null,
  restartRequired: false,
  revision: 0,
};
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(zcodeDeviceApi.state).mockResolvedValue(empty);
});
it("imports explicitly, preserves backend revision and shows saved IDs", async () => {
  vi.mocked(zcodeDeviceApi.import).mockResolvedValue({
    ...empty,
    devices: [id],
    revision: 1,
  });
  render(<ZCodeDeviceManager dataRoot="/synthetic/library" />);
  await waitFor(() => expect(screen.getByRole("combobox")).toBeEnabled());
  expect(zcodeDeviceApi.import).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Device IDs to import"), {
    target: { value: id },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import device IDs" }));
  await waitFor(() =>
    expect(zcodeDeviceApi.import).toHaveBeenCalledWith(
      "/synthetic/library",
      id,
      0,
    ),
  );
  expect(zcodeDeviceApi.select).not.toHaveBeenCalled();
});
it("reads a JSON file without importing or selecting until requested", async () => {
  render(<ZCodeDeviceManager />);
  const text = JSON.stringify({ profiles: [{ device_mid: id }] });
  fireEvent.change(screen.getByLabelText("Device ID file"), {
    target: {
      files: [new File([text], "devices.json", { type: "application/json" })],
    },
  });
  await waitFor(() =>
    expect(screen.getByLabelText("Device IDs to import")).toHaveValue(text),
  );
  expect(zcodeDeviceApi.import).not.toHaveBeenCalled();
  expect(zcodeDeviceApi.select).not.toHaveBeenCalled();
});
it("keeps the current selection and draft on a rejected import", async () => {
  vi.mocked(zcodeDeviceApi.import).mockRejectedValue("invalid");
  render(<ZCodeDeviceManager />);
  await waitFor(() => expect(screen.getByRole("combobox")).toBeEnabled());
  fireEvent.change(screen.getByLabelText("Device IDs to import"), {
    target: { value: "invalid-input" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import device IDs" }));
  await screen.findByRole("alert");
  expect(screen.getByLabelText("Device IDs to import")).toHaveValue(
    "invalid-input",
  );
  expect(zcodeDeviceApi.select).not.toHaveBeenCalled();
});
it("shows that a persisted selection takes effect after restart", async () => {
  vi.mocked(zcodeDeviceApi.state).mockResolvedValue({
    ...empty,
    devices: [id],
    selectedDeviceMid: id,
    restartRequired: true,
  });
  render(<ZCodeDeviceManager />);
  expect(await screen.findByRole("status")).toHaveTextContent(
    "Restart LoongPort",
  );
});
it("ignores a late mutation response after changing library context", async () => {
  let finish!: (value: ZCodeDeviceState) => void;
  vi.mocked(zcodeDeviceApi.import).mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const view = render(<ZCodeDeviceManager dataRoot="/synthetic/first" />);
  await waitFor(() => expect(screen.getByRole("combobox")).toBeEnabled());
  fireEvent.change(screen.getByLabelText("Device IDs to import"), {
    target: { value: id },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import device IDs" }));
  view.rerender(<ZCodeDeviceManager dataRoot="/synthetic/second" />);
  await waitFor(() => expect(screen.getByRole("combobox")).toBeEnabled());
  finish({
    ...empty,
    devices: [id],
    selectedDeviceMid: id,
    restartRequired: true,
  });
  await waitFor(() =>
    expect(screen.getByLabelText("Device IDs to import")).toHaveValue(""),
  );
  expect(screen.queryByRole("status")).not.toBeInTheDocument();
});

it("selects an imported ID explicitly and reports the restart boundary", async () => {
  vi.mocked(zcodeDeviceApi.state).mockResolvedValue({
    ...empty,
    devices: [id],
    revision: 3,
  });
  vi.mocked(zcodeDeviceApi.select).mockResolvedValue({
    ...empty,
    devices: [id],
    selectedDeviceMid: id,
    revision: 4,
    restartRequired: true,
  });
  render(<ZCodeDeviceManager dataRoot="/synthetic/library" />);
  const trigger = await screen.findByRole("combobox");
  await waitFor(() => expect(trigger).toBeEnabled());
  fireEvent.keyDown(trigger, { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: id }));
  await waitFor(() =>
    expect(zcodeDeviceApi.select).toHaveBeenCalledWith(
      "/synthetic/library",
      id,
      3,
    ),
  );
  expect(await screen.findByRole("status")).toHaveTextContent(
    "Restart LoongPort",
  );
});
it("can retry an unavailable initial database read", async () => {
  vi.mocked(zcodeDeviceApi.state)
    .mockRejectedValueOnce("unavailable")
    .mockResolvedValue(empty);
  render(<ZCodeDeviceManager />);
  fireEvent.click(await screen.findByRole("button", { name: "Retry" }));
  await waitFor(() => expect(screen.getByRole("combobox")).toBeEnabled());
});
