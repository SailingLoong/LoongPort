# DeepSeek 响应 fixture

- **抓取日期**：2026-08-03（真机 Playwright 登录 + 裸 curl 复核）
- **bundle commit-id**：`a274378`（`main.50ec61b52a.js`）
- **脱敏了什么**：明文 sk 换成 `sk-` + 32 个 0（**保持 35 字符、零星号**，
  否则 `validate_plaintext_key` 那道闸失去判别力）；脱敏值保持同长 35 字符 + 26 个星号；
  `tracking_id` 换成全零 UUID；账号名换成占位。
- **绝不入库**：真实 token、真实明文 sk。

⚠️ 官网改接口后这些 fixture 会与线上不符。重新抓取时**连带更新上面的日期与
commit-id**，否则下一个读者无法判断证据的新旧。
