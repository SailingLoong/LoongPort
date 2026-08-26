# crowd-metrics

中转站**实测数据共建**的聚合 Worker：客户端（LoongPort 桌面端）把本地
`proxy_request_logs` 聚合出的**小时级聚合桶**上传到这里，Worker 落 D1、
定时聚合成 k-匿名后的公共快照，客户端与网站共用同一份快照。

## 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v1/ingest` | 客户端上传小时聚合桶（校验 + 每 IP 限流 20 次/时 + 幂等覆盖） |
| GET | `/v1/snapshot` | 公共快照（CORS `*`、CDN `max-age=60`；KV 命中，冷启动现算兜底） |
| GET | `/healthz` | 探活 |

Cron（每 5 分钟）：D1 重算快照 → 写 KV；清理 30 天前的原始桶与 2 天前的限流计数。

## 隐私边界（硬约束，改动前先读 `src-tauri/src/crowd/` 的模块文档）

- **只有聚合指标**：站点 host、请求数、错误数、TTFT 直方图、token 计数、微美元花费。
  没有提示词、密钥、账号、时间戳级明细。
- **k-匿名**：任何发布的聚合 ≥ 3 个独立来源（`MIN_SOURCES`）；来源是客户端**每日轮换**
  的随机 id，不是持久安装标识。
- **接收端不记 IP**：限流只存 IP 的 SHA-256（`upload_ip_hour`），保留 2 天。
- 原始桶保留 30 天后删除；KV 里只有 k-匿名后的快照。

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

# POST 冒烟（写入的是 staging 的 D1，且单来源永远过不了 k-匿名，不会出现在快照里）：
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

- POST：活跃用户每天几十次 flush + cron 288 次调用 ≪ Workers 免费 10 万请求/天
- D1：每天几万行写 ≪ 10 万行/天；读只有 cron（每 5 分钟一次窗口查询）
- KV：只有 cron 写快照，288 次/天 < 免费档 1 千写/天
- 到万级 DAU 再上 $5/月付费档

## TTFT 桶边界是跨语言共享常量

`src/bins.ts` 与客户端 `src-tauri/src/crowd/bins.rs` 必须一致（服务端按位置求和）。
Rust 侧有一条解析本文件比对的闸测试；改任何一边都会红。
