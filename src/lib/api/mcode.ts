import { invoke } from "@tauri-apps/api/core";

export interface McodeBackup {
  id: string;
  revision: string;
}

export const mcodeApi = {
  getBackup: (): Promise<McodeBackup | null> => invoke("get_mcode_backup"),
  restoreBackup: (backup: McodeBackup): Promise<boolean> =>
    invoke("restore_mcode_backup", {
      id: backup.id,
      revision: backup.revision,
    }),
};
