# 站点自报调用配置（站长指南）

中转站站长可以在自己域名下放一份 JSON 声明，告诉 LoongPort「在我这个站的各平台上，
用什么默认模型和调用参数最合适」。用户添加你的站点后开箱即用的就是这套配置，
站长维护的永远比客户端内置默认更贴近实际。

完整格式示例：[site-config.example.json](../site-config.example.json)
（schema 字段参考 cc-switch 各 app 可写面、命名参考 sub2api 习惯）。

## 放哪里

默认约定路径（推荐，用户零操作）：

```text
https://<你的域名>/.well-known/loongport.json
```

用户登录/添加站点时 LoongPort 会自动探测这个路径，拉到即应用。没有这个文件
就静默跳过，完全不影响站点正常接入。

也可以用任意 HTTPS URL 或直接把 JSON（或它的 base64）发给用户，
用户在你的站点行上点「导入站点配置」粘贴即可（见下）。

## 格式

```json
{
  "schema_version": 1,
  "site_origin": "https://api.example.com",
  "platforms": {
    "anthropic": { "env": { "ANTHROPIC_MODEL": "claude-fable-5.1" } },
    "openai": { "model": "gpt-5.6-codex", "model_context_window": 272000 },
    "gemini": { "env": { "GEMINI_MODEL": "gemini-3-pro" } },
    "grok": { "defaultModel": "grok-5" }
  }
}
```

- `site_origin` 必须与用户正在添加的站点同域（允许子域差异），否则按防钓鱼拒收；
- `platforms` 的键是平台标识（`anthropic` / `openai` / `gemini` / `grok`）；
- 每个平台的段内字段就是对应 CLI 配置文件的原生键：anthropic/gemini 走 `env`，
  openai 是 codex `config.toml` 的键，grok 是其配置 JSON 的键。站长抄一份
  「这台 CLI 在我站上跑得最好的配置」进来即可；
- **键值 `null` 表示显式删掉这个键**（比如想去掉客户端内置写死的推理档位）；
- 未声明的平台不受影响（客户端用内置默认）。

## 安全边界（会被拦下的键）

声明只携带**调用参数**。以下三类键无论放在哪一层都会被丢弃：

- 执行面：`hooks`、`mcpServers` / `mcp_servers`、`permissions`、
  `shell_environment_policy`、`notifications`、`sandbox*`、`statusLine`；
- 进程环境：`PATH`、`LD_*` / `DYLD_*`、代理指向（`HTTP_PROXY` 等）等 env 键；
- 端点与凭证：`base_url`、`api_key`、`auth` 等——这两样永远来自用户自己的
  登录与客户端建档，声明不开放覆盖。

清单之外的键默认放行：CLI 将来新增调用参数，无须等 LoongPort 发版就能配。

## 用户侧如何生效

- **自动**：登录/添加站点成功、首次建立档位时自动应用（之后不重复覆盖，
  避免冲掉用户的调整）；
- **手动**：站点行 hover 出现的「导入站点配置」里粘贴 URL / JSON / base64，
  显式同步站长的最新配置；同一个弹窗里可「恢复默认」一键退回客户端内置值。

## 怎么验证

把文件放到约定路径后，用你的站点在 LoongPort 里走一遍「导入站点配置」
（粘贴 `https://<域名>/.well-known/loongport.json`）：格式或同源有任何问题，
弹窗会给出具体原因；应用成功会提示应用到了几条接入配置。
