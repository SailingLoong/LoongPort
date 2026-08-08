import { MANAGED_PROVIDER_ID_PREFIX } from "./constants";

/**
 * 「这条 provider 是 LoongPort 托管的」—— 前端侧判据。
 *
 * ## 为什么不是一个 `startsWith` 就够
 *
 * 前缀判的是**形状**，而判据要回答的是「这条记录**是我们生成的**吗」（**来源**）。
 * 两者不等价，而有一条真实可达的路径能让用户的 provider 撞上前缀：**live config
 * 导入**（Rust 侧 `services/provider/live.rs` 的三个 `import_*_providers_from_live`）
 * 的 provider id 就是用户自己 CLI 配置文件里的 key（`~/.config/opencode/opencode.json`
 * 等），且那三处绕过命令层的 `reject_if_managed`、启动时无条件跑。
 *
 * ⚠️ **不是「表单里手填」那条** —— `add_provider` 早就有 `reject_if_managed`，
 * 在表单里填 `loongport-mine` 会当场被拒、建不出记录。
 *
 * 误判的后果对用户是死局：那条 provider 从列表里消失、编辑与删除都被后端守卫拦下，
 * 而错误文案指向一个没有它的区。
 *
 * ## ⚠️ 这份判据必须与 Rust 侧同形
 *
 * 权威定义在 `src-tauri/src/relay/managed.rs` 的 `is_managed`。跨语言没法共享
 * 实现，而**两侧不一致不会报错，只会换一种死局**：只有一边收紧时，用户的 provider
 * 要么「看不见但能改」，要么「看得见但改不了」。
 *
 * 三道闸合起来守这件事：
 * - `managed.rs` 的 `prefix_matches_the_frontend_copy` —— 前缀常量的字面比对
 * - `managed.rs` 的 `both_generators_produce_ids_the_guard_recognizes` —— 把 Rust
 *   判据钉在两个**生成器**上（生成端改了格式就红）
 * - `tests/config/managedProviderId.test.ts` —— 本函数的行为，用例与 Rust 那边
 *   `user_authored_ids_...` 一一对应
 */

/** vendor（官网直连）那支在前缀之后多加的一段。事实源：`vendor::provision::provider_id_for`。 */
const VENDOR_SEGMENT = "vendor-";

/** 派生 id 尾部那段 hex 的长度 —— 两个生成端都取 16 位（`format!("{:.16x}")`）。 */
const HEX_LEN = 16;

/**
 * 恰好 `HEX_LEN` 位**小写** hex。
 *
 * 大小写敏感是有意的：Rust 的 `{:x}` 恒产出小写，放行大写会把判据重新放宽到
 * 用户填得出的形状上。
 */
const HEX_PATTERN = new RegExp(`^[0-9a-f]{${HEX_LEN}}$`);

export function isManagedProviderId(providerId: string): boolean {
  if (!providerId.startsWith(MANAGED_PROVIDER_ID_PREFIX)) return false;
  const rest = providerId.slice(MANAGED_PROVIDER_ID_PREFIX.length);
  // vendor 那支多一段；剥掉之后两支的尾部形状相同。
  const hex = rest.startsWith(VENDOR_SEGMENT)
    ? rest.slice(VENDOR_SEGMENT.length)
    : rest;
  return HEX_PATTERN.test(hex);
}
