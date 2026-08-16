/**
 * 自动模式「一次性授权」的共享存储。
 *
 * 开启自动模式（尤其是一键开启：顺带启动本地路由并接管该 CLI 的配置）属于
 * 明确授权动作，首次要弹确认框；同意过一次就不再问。这是纯 UX 状态，
 * 不值得进后端设置 —— 与故障转移的 `failoverConfirmed`（后端设置字段）刻意
 * 不同做法，别搬过去。设置页卡片与顶栏开关共用这一份，别各存一个 key。
 */
export const AUTO_MODE_CONFIRMED_STORAGE_KEY = "loongport.autoModeConfirmed";

export function hasConfirmedAutoMode(): boolean {
  return localStorage.getItem(AUTO_MODE_CONFIRMED_STORAGE_KEY) === "true";
}

export function markAutoModeConfirmed(): void {
  localStorage.setItem(AUTO_MODE_CONFIRMED_STORAGE_KEY, "true");
}
