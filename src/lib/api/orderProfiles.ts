import { invoke } from "@tauri-apps/api/core";

/** 档位顺序配置档：命名的顺序快照（详见 `src-tauri/src/commands/order_profiles.rs`）。 */
export interface OrderProfile {
  name: string;
  providerIds: string[];
}

/** 配置档列表 + 当前配置文件名：「应用此顺序」默认保存进当前档。 */
export interface OrderProfilesState {
  profiles: OrderProfile[];
  current: string;
}

export const orderProfilesApi = {
  list: (appType: string): Promise<OrderProfilesState> =>
    invoke("get_order_profiles", { appType }),
  /** 保存命名快照（同名覆盖）；应用成功才改变当前配置档。 */
  save: (appType: string, name: string, providerIds: string[]): Promise<void> =>
    invoke("save_order_profile", { appType, name, providerIds }),
  /** 重命名配置档；目标名已存在会拒绝。 */
  rename: (appType: string, from: string, to: string): Promise<void> =>
    invoke("rename_order_profile", { appType, from, to }),
  remove: (appType: string, name: string): Promise<void> =>
    invoke("delete_order_profile", { appType, name }),
  /** 自带保存对话框；返回写入路径，用户取消返回 null。 */
  export: (appType: string): Promise<string | null> =>
    invoke("export_order_profiles", { appType }),
  /** 自带打开对话框，同名单档覆盖；返回导入条数，取消返回 null。 */
  import: (appType: string): Promise<number | null> =>
    invoke("import_order_profiles", { appType }),
};
