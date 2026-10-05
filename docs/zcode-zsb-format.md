# ZCode .zsb 固定格式与兼容边界

本版格式来源固定为 [pjpv/zcode-switch f34225686dfef05d84c256a56f868719248f15ff](https://github.com/pjpv/zcode-switch/tree/f34225686dfef05d84c256a56f868719248f15ff)。这里记录源码契约与实施门槛，不表示 LoongPort 已完成导入。

外层（cipher.rs）是 `format: zsw-accounts-bundle`、整数 `version: 1`；`kdf` 仅接受 `algo: pbkdf2-hmac-sha256`、`iters: 100000`、标准 Base64 编码的 16-byte salt。`cipher` 仅接受 `algo: aes-256-gcm`、标准 Base64 编码的 12-byte nonce、16-byte tag 和非空 data。认证运算使用 ciphertext 后接 tag；不使用额外 AAD。其他算法、版本或参数拒绝，不能让包指定任意 KDF 工作量。

内层（store.rs 的 export_bundle_value）是 `format: zcode-accounts-bundle`、整数 `version: 2`、`exportedAt` 与 `accounts`。每条导出项只有 `name`、`createdAt`、`credentials`、`config`。源包没有来源平台、ZCode version/build 或 personal scope 的可靠声明。名称和导出时间不能代替稳定身份、令牌新鲜度或兼容性证据；config 不直接写入官方配置。

LoongPort 门槛：单包最多 10 MiB、50 个账号；先做有界读取和严格字段/重复键校验，再解密到受清零保护的内存。目标构建必须经过既有 admission；内层原有 `enc:v1` 必须在明确的目标 key context 下核验。外层认证成功只证明口令封装完整，不能证明跨机凭据可用。缺失来源条件、未知 schema 或无法确认身份的项显示具体阻塞，不能作为可切换账号入库。不会把账号包内容、口令或原始会话传入聊天、日志或前端预览。

当前只有纯参数/资源边界测试通过；严格 JSON/base64 解析、PBKDF2/AES-GCM、目标 schema/身份、重复决策、vault 原子导入和隔离原生兼容验收尚待完成。所有导入最终都不激活账号，不写 ZCode 会话。

参考源码：

- [cipher.rs](https://github.com/pjpv/zcode-switch/blob/f34225686dfef05d84c256a56f868719248f15ff/src-tauri/src/cipher.rs)，下载 SHA-256 `daedbc02dddd457f9648f6924e9ab9f1e3dcd221738d797e4503db60c3a1560c`。
- [store.rs](https://github.com/pjpv/zcode-switch/blob/f34225686dfef05d84c256a56f868719248f15ff/src-tauri/src/store.rs)，下载 SHA-256 `a458b6283e4b4be7cab4e2444cd9bac667d825fc6aa9be97083a5ff65e73d767`。
- [MIT LICENSE](https://github.com/pjpv/zcode-switch/blob/f34225686dfef05d84c256a56f868719248f15ff/LICENSE)，下载 SHA-256 `b60434437a13233229ce3858564410413beff877d905c2b11bfb8600df738e9e`；许可全文归档于 `docs/licenses/pjpv-zcode-switch-MIT.txt`。
