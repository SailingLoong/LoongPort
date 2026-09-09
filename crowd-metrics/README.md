# crowd-metrics

中转站**实测数据共建**的聚合 Worker：客户端（LoongPort 桌面端）把本地
`proxy_request_logs` 聚合出的**小时级聚合桶**上传到这里，Worker 落 D1、
定时聚合成 k-匿名后的公共快照，客户端与网站共用同一份快照。

2026-09-09 起同一 Worker 还承载**匿名使用统计**（`relay::stats`，安装量/版本/OS/
在用站点）：写入型端点 `/v1/ping`，只落 D1、**无公开读端点**，与公开快照互不相通。

## 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v1/ingest` | 客户端上传小时聚合桶（校验 + 每 IP 限流 20 次/时 + 幂等覆盖） |
| POST | `/v1/ping` | 客户端匿名使用统计上报（安装 id + 版本 + OS + 站点域名；只落 D1，永不公开；与 ingest 共享每 IP 20 次/时限流） |
| GET | `/v1/snapshot` | 公共快照（CORS `*`、CDN `max-age=60`；KV 命中，冷启动现算兜底） |
| GET | `/healthz` | 探活 |

### 错误率分母（errSamples，2026-09-10 起）

桶自该日起收客户端的**全部**用量行（本地路由 + 直连的 session 回填），
错误率分母随之从 `samples` 换成 `errSamples`（错误可观测样本数 = proxy 观测行）：

- 直连桶（`errSamples = 0`）`errRate` 为 null（无错误观测 ≠ 零失败）；
- D1 旧行 `err_samples IS NULL`（新列引入前）聚合回退 `samples` —— 当时桶只收
  proxy 行，`samples` 就是完整分母，旧行为保持；
- 发布顺序硬约束：**先部署 Worker（本仓），再发客户端** —— 旧 Worker 会忽略
  `errSamples` 字段并把 session 行算进 `samples`，稀释公开错误率。

新鲜度 **GET 自愈**（2026-08-26 起）：快照超过 10 分钟陈旧时，下一次 GET 在请求路径里现算重写（前面的 CDN 60s 缓存把现算限流到约每分钟一次）；清理（30 天原始桶 / 2 天限流计数）折叠进现算路径、按小时时间闸执行。cron（每 10 分钟）是常规重算通道：2026-08-26 曾实证它不触发、系统单靠自愈运转；2026-09-07 KV 写入配额对账证实 cron 已恢复实跑（每 5 分钟 × 2 键 ≈ 576 写/天，触发免费档 50% 告警线），遂放宽到 10 分钟，与自愈新鲜窗对齐。

## 隐私边界（硬约束，改动前先读 `src-tauri/src/crowd/` 的模块文档）

- **只有聚合指标**：站点 host、请求数、错误数、TTFT 直方图、token 计数、微美元花费。
  没有提示词、密钥、账号、时间戳级明细。
- **k-匿名**：任何发布的聚合需 ≥ `MIN_SOURCES` 个独立来源（来源是客户端**每日轮换**
  的随机 id，不是持久安装标识）。⚠️ **2026-09-06 起门槛临时放开为 1/1**
  （参与用户还少，3×2 让实测页必然空白；单源数据实质是单个用户的使用画像，
  属有意识的临时让步）——恢复条件与随改清单见 `src/aggregate.ts` 常量注释。
- **接收端不记 IP**：限流只存 IP 的 SHA-256（`upload_ip_hour`），保留 2 天。
- 原始桶保留 30 天后删除；KV 里只有 k-匿名后的快照。
- **使用统计（`/v1/ping` → `stats_installs`）永不公开**：没有读端点、不进快照/KV，
  维护者经 `wrangler d1 execute --remote` 直查。一行 = 一个安装（随机 UUID，与
  device_id / crowd source id 永不交叉），**180 天**未见活动即删除（接收端
  「设保留期」义务）。隐私口径的唯源是 `src-tauri/src/relay/stats.rs` 模块文档。

## 维护者怎么查使用统计（示例）

```bash
cd crowd-metrics
# 活跃安装（近 7 天上报过）
npx wrangler d1 execute loongport-metrics --remote \
  --command "SELECT COUNT(*) FROM stats_installs WHERE last_seen > strftime('%s','now') - 7*86400"
# 版本分布 / 平台分布
npx wrangler d1 execute loongport-metrics --remote \
  --command "SELECT app_version, os, COUNT(*) n FROM stats_installs GROUP BY 1,2 ORDER BY n DESC"
# 在用站点（站点列表存 JSON 数组，json_each 展开）
npx wrangler d1 execute loongport-metrics --remote \
  --command "SELECT json_each.value host, COUNT(*) n FROM stats_installs, json_each(site_hosts) GROUP BY 1 ORDER BY n DESC"
```

## 首次资源创建（一次性）

```bash
cd crowd-metrics
npx wrangler login   # 或用 CLOUDFLARE_API_TOKEN

npx wrangler d1 create loongport-metrics
# 输出 database_id → 填进 wrangler.jsonc 的 REPLACE_WITH_D1_ID

npx wrangler kv namespace create SNAPSHOT
# 输出 id → 填进 wrangler.jsonc 的 REPLACE_WITH_KV_ID
```

可选：自定义域 `metrics.loongport.dev`（zone 在同账号下，DNS 加一条 CNAME 或在
dashboard 给 Worker 绑 custom domain）。客户端常量直接写正式域名即可。

## 部署

```bash
CLOUDFLARE_API_TOKEN=… ./deploy.sh
```

deploy.sh 会：拒绝占位 id → 重放 `schema.sql`（幂等）→ `wrangler deploy` → 线上验证。

## 本地与 staging 验证

本机 workerd 沙箱起不了监听（维护者机器已知问题），集成验证走 **staging Worker**：

```bash
npx wrangler deploy --name loongport-metrics-dev   # 独立名字，不碰生产
./verify.sh https://loongport-metrics-dev.<account-subdomain>.workers.dev

# POST 冒烟（写入的是 staging 的 D1；门槛临时 1/1 期间单来源也会进快照，恢复 3/2 后不会）：
curl -X POST https://…/v1/ingest -H 'content-type: application/json' -d '{
  "version": 1,
  "sourceId": "00112233445566778899aabbccddeeff",
  "hours": [{
    "hour": "2026-08-26T07Z", "site": "verify.example", "app": "claude",
    "samples": 10, "errors": 1,
    "ttftBins": [0,8,2,0,0,0,0,0,0,0,0,0], "ttftCount": 10,
    "inputTokens": 1000, "outputTokens": 500,
    "cacheReadTokens": 300, "cacheCreationTokens": 100, "costUsdMicros": 12345
  }]
}'
# → {"accepted":1}；等待下一个 cron 周期后 GET /v1/snapshot 应仍不含 verify.example
```

纯逻辑（校验/聚合/分位数）全部在 vitest 里：`pnpm check`。

## 免费额度账（千级 DAU）

- POST：活跃用户每天几十次 flush + cron 144 次调用 ≪ Workers 免费 10 万请求/天
- D1：每天几万行写 ≪ 10 万行/天；读只有 cron（每 10 分钟一次窗口查询）
- KV：重算写 snapshot+trend 两键 + 每小时清理标记 ≈ 312 次/天 < 免费档 1 千写/天的 50% 告警线
- 到万级 DAU 再上 $5/月付费档

## TTFT 桶边界是跨语言共享常量

`src/bins.ts` 与客户端 `src-tauri/src/crowd/bins.rs` 必须一致（服务端按位置求和）。
Rust 侧有一条解析本文件比对的闸测试；改任何一边都会红。
