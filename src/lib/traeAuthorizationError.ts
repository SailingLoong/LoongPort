// Presentation only: these exact safe codes are serialized by entry::Error.
// Never inspect native error messages, HTTP bodies, paths, or nested payloads.
export const TRAE_AUTHORIZATION_ERROR_CODES = [
  "cancelled",
  "busy",
  "invalidSelection",
  "unsupported",
  "storageUnavailable",
  "authorizationUnavailable",
  "publicationUnknown",
  "expired",
] as const;

export type TraeAuthorizationFailure =
  | (typeof TRAE_AUTHORIZATION_ERROR_CODES)[number]
  | "receiptUnknown"
  | "unknown";

export function traeAuthorizationFailure(
  error: unknown,
): TraeAuthorizationFailure {
  return (
    TRAE_AUTHORIZATION_ERROR_CODES.find((code) => code === error) ?? "unknown"
  );
}

export const TRAE_AUTHORIZATION_FAILURE_MESSAGES: Record<
  TraeAuthorizationFailure,
  string
> = {
  cancelled:
    "授权流程已结束或不再有效，不能据此确认账号是否已保存。请重新载入列表核对。",
  busy: "本地账号操作正忙，本次操作未完成。请先重新载入列表核对。",
  invalidSelection:
    "所选安装文件未通过读取或校验。这不代表账号不同；请检查同一官方安装的文件，并先核对列表。",
  unsupported:
    "当前操作不受支持。请核对官方桌面版与账号区域，不要改用其他安装的凭据。",
  storageUnavailable:
    "无法访问或确认本地账号存储。请检查凭据保管库状态，再重新载入列表核对。",
  authorizationUnavailable:
    "本次授权未能确认完成，现有信息不足以判断具体原因，也不能据此认定账号不同。请先重新载入列表核对。",
  publicationUnknown:
    "本次保存回执未确认，账号可能已保存在本地。请先重新载入列表核对，不要重复授权。",
  expired:
    "本次授权流程已失效。请先核对列表；如仍需导入，再重新选择文件并确认授权。",
  receiptUnknown:
    "收到的授权回执无法确认，账号可能已保存。这不代表账号不同；请先重新载入列表核对，不要重复授权。",
  unknown:
    "未能确认本次操作结果，具体原因未知。请先重新载入列表核对，不要重复授权；本次不会自动重试。",
};
