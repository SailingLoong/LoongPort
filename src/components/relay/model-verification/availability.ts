/**
 * 模型验证功能总开关（前端侧，模块唯一收口处）。
 *
 * `false` = 整个验真模块下线：Provider 不拉取 summaries，入口/徽章组件
 * 一律不渲染 —— 调用方「拿不到验真」即默认不展示，不需要各自判断。
 * 后端在读写下两侧同步收口（`relay/model_verification::MODEL_VERIFICATION_ENABLED`）。
 *
 * 2026-09-05 曾因判定规则系统性误判整体下线（v6.14.1）；规则修正
 * （模型名前缀匹配、官方流式用量语义、SSE 归一化、证据去重等，
 * 见 design 仓 spec-模型验证判定规则修正.md）后恢复，下线前存量报告
 * 由后端按 rules_version=2 过滤作废。
 */
export const MODEL_VERIFICATION_ENABLED = true;
