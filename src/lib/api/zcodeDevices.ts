import { invoke } from "@tauri-apps/api/core";

export interface ZCodeDeviceState {
  devices: string[];
  selectedDeviceMid: string | null;
  effectiveDeviceMid: string | null;
  restartRequired: boolean;
  revision: number;
}
export const zcodeDeviceApi = {
  state: (dataRoot?: string) =>
    invoke<ZCodeDeviceState>("get_zcode_device_state", { dataRoot }),
  import: (dataRoot: string | undefined, input: string, revision: number) =>
    invoke<ZCodeDeviceState>("import_zcode_device_ids", {
      dataRoot,
      input,
      revision,
    }),
  select: (
    dataRoot: string | undefined,
    deviceMid: string | null,
    revision: number,
  ) =>
    invoke<ZCodeDeviceState>("select_zcode_device_id", {
      dataRoot,
      deviceMid,
      revision,
    }),
};
