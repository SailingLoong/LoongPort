/**
 * 模型验证功能总开关（前端侧，模块唯一收口处）。
 *
 * `false` = 整个验真模块下线：Provider 不拉取 summaries，入口/徽章组件
 * 一律不渲染 —— 调用方「拿不到验真」即默认不展示，不需要各自判断。
 * 后端在读写下两侧同步收口（`relay/model_verification::MODEL_VERIFICATION_ENABLED`）。
 *
 * 下线原因（2026-09）：主动探针的判定规则对部分合规上游存在系统性误判 ——
 * 模型标识按请求名全等比对，撞上响应回显带日期快照模型名的上游即假异常；
 * 流式用量一致性检查要求了官方流式协议不保证的字段组合。判定规则修正后
 * 恢复：本开关与后端同名开关一起置回 `true`，并清空或按 `rules_version`
 * 过滤下线前落库的旧报告（读侧当前不过滤版本）。
 */
export const MODEL_VERIFICATION_ENABLED = false;
