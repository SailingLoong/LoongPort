## Summary / 概述

<!-- Briefly describe what this PR does and why. / 简要描述这个 PR 做了什么以及为什么。 -->

## Delivery / 交付记录

<!-- Record actual owners and evidence. For small corrections, use a short paragraph with these fields; use N/A with a reason where a stage does not apply. Do not copy private records or chat history. / 填实际责任与证据；简单更正可用包含这些字段的短段落。不适用的阶段写 N/A 及原因，不复制私有档案或聊天记录。 -->

| Field / 字段 | Record / 记录 |
| --- | --- |
| Responsibility / 责任 | Input/scope owner; implementer; independent reviewer; acceptance/release owner / 输入范围、实施、独审、验收发布的实际负责人 |
| Inputs and approval / 输入与批准 | Public issue/spec or repository document; exact approved version and scope / 公开议题或本仓文件、获批版本及范围 |
| Deliverables / 交付物 | Resulting behavior and relevant files / 结果行为与相关文件 |
| Validation / 验证 | Commands/results, exact tested SHA, CI link, independent review conclusion / 命令结果、测试 SHA、CI 与独审结论 |
| Gaps / 缺口 | Remaining gates, blockers, owner and next step / 未过门禁、阻塞、负责人和下一步 |
| Scope changes / 裁剪 | Removed/deferred scope, reason and approval status; otherwise none / 剔除或延后内容、原因及批准状态；无则写无 |
| Delivery stage / 交付阶段 | Local / exact-SHA CI / merged / released / native or live acceptance, separately / 本地、精确 SHA CI、合并、发布、原生或线上验收分别记录 |

<!-- Rules remain in CLAUDE.md and CONTRIBUTING.md. Packaging gates: docs/tauri-packaging-compatibility.md. Example: docs/zcode-quick-account-v1-delivery.md. / 规则来源仍是 CLAUDE.md 与 CONTRIBUTING.md；打包门禁和交付样例见上述本仓文件。 -->

## Related Issue / 关联 Issue

<!-- Link the related issue. Use "Fixes #123" to auto-close it when merged. -->
<!-- 关联相关 Issue。使用 "Fixes #123" 可在合并时自动关闭。 -->

Fixes #

## Screenshots / 截图

<!-- If applicable, add before/after screenshots. / 如有需要，请添加修改前后的截图。 -->

| Before / 修改前 | After / 修改后 |
|-----------------|---------------|
|                 |               |

## Checklist / 检查清单

- [ ] `pnpm typecheck` passes / 通过 TypeScript 类型检查
- [ ] `pnpm format:check` passes / 通过代码格式检查
- [ ] `cargo clippy` passes (if Rust code changed) / 通过 Clippy 检查（如修改了 Rust 代码）
- [ ] Updated i18n files if user-facing text changed / 如修改了用户可见文本，已更新国际化文件

- [ ] Delivery record names the actual owner, inputs/approved scope, evidence and remaining gaps / 交付记录包含实际责任、输入与批准范围、证据及剩余缺口
- [ ] Independent review and applicable native/cross-platform gates are recorded as passed, pending or N/A with evidence / 独审及相关原生与跨平台门禁有明确状态和证据
