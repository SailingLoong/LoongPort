import { invoke } from "@tauri-apps/api/core";

/** 档位顺序配置档：命名的顺序快照（详见 `src-tauri/src/commands/order_profiles.rs`）。 */
export interface OrderProfile {
  name: string;
  providerIds: string[];
}

export const orderProfilesApi = {
  list: (appType: string): Promise<OrderProfile[]> =>
    invoke("get_order_profiles", { appType }),
  save: (appType: string, name: string, providerIds: string[]): Promise<void> =>
    invoke("save_order_profile", { appType, name, providerIds }),
  remove: (appType: string, name: string): Promise<void> =>
    invoke("delete_order_profile", { appType, name }),
  /** 自带保存对话框；返回写入路径，用户取消返回 null。 */
  export: (appType: string): Promise<string | null> =>
    invoke("export_order_profiles", { appType }),
  /** 自带打开对话框，同名单档覆盖；返回导入条数，取消返回 null。 */
  import: (appType: string): Promise<number | null> =>
    invoke("import_order_profiles", { appType }),
};
