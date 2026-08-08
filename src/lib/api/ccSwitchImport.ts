import { invoke } from "@tauri-apps/api/core";

/** 一条不导入的 cc-switch provider（同指纹跳过，或同站点归入运营商组）。 */
export interface SkippedProvider {
  name: string;
  appType: string;
}

export interface ProviderPlan {
  /** 会导入的 provider 条数（含取不到指纹、原样导入的那些）。 */
  willImport: number;
  /** 与托管档位同指纹而跳过的。 */
  skipped: SkippedProvider[];
  /** 站点命中已登录运营商（base_url 同源）而归入运营商组维护、不导入的。 */
  mergedToOperator: SkippedProvider[];
  /** 取不到指纹的条数（这些原样导入、不参与冲突合并）。 */
  cannotFingerprint: number;
}

/** 从 cc-switch 导入的预览（后端 `get_cc_switch_import_preview`）。 */
export interface CcSwitchImportPreview {
  /** `~/.cc-switch/cc-switch.db` 是否存在 —— 前端据此决定显不显入口。 */
  sourceExists: boolean;
  /** 源库 schema 版本。 */
  sourceVersion: number | null;
  providers: ProviderPlan;
  mcpServers: number;
  prompts: number;
  skills: number;
  notes: string[];
}

/** 导入结果报告（后端 `import_from_cc_switch`）。 */
export interface CcSwitchImportReport {
  success: boolean;
  /** 导入前自动建的备份文件名（空串 = 没走导入）。 */
  backupId: string;
  providersImported: number;
  providersSkipped: SkippedProvider[];
  /** 站点命中已登录运营商、归入运营商组维护而未导入的。 */
  operatorsMerged: SkippedProvider[];
  mcpImported: number;
  promptsImported: number;
  skillsImported: number;
  warnings: string[];
}

export const ccSwitchImportApi = {
  /**
   * 预览：读 `~/.cc-switch/cc-switch.db`（只读）+ 本地托管档位，算冲突。不写任何东西。
   */
  getPreview: async (): Promise<CcSwitchImportPreview> => {
    return invoke("get_cc_switch_import_preview");
  },

  /**
   * 一键导入（拷贝，不动 cc-switch 那边）。返回报告。
   */
  import: async (): Promise<CcSwitchImportReport> => {
    return invoke("import_from_cc_switch");
  },
};
