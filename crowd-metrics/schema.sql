-- crowd-metrics D1 schema（幂等：CREATE IF NOT EXISTS，deploy.sh 每次部署都重放）。
-- 存量表的列迁移（asn / ua_trusted）在 deploy.sh 里带护栏地 ALTER，见部署脚本注释。

-- 原始小时聚合桶。一行 = 某来源在某小时对某站点某 app 的聚合指标。
-- PK (hour, site, app, source) + INSERT OR REPLACE = 幂等：
-- 客户端对同一小时总是重发**全量**桶（从本地 SQLite 重算），重试/补发覆盖而非累加。
-- ⚠️ 这里只有聚合指标 —— 客户端不上传原始请求，服务端也永远拿不到。
-- hour 形如 '2026-08-26T07Z'（UTC，定宽 → 字典序即时间序，PK 前缀扫描即窗口查询）。
--
-- asn / ua_trusted 是反作弊维度（2026-08-26 加固）：
-- - asn 由 Cloudflare 边缘从请求本身带出（request.cf.asn），**客户端伪造不了**；
--   聚合的发布门槛要求来源横跨 ≥2 个 ASN —— 一个 VPS/一个网络出口刷不动。
-- - ua_trusted 只是把「明显不是真客户端」的上传挡在 k-匿名计数外（开源可查、
--   可伪造，防的是最懒的脚本，不是防御本体）。
CREATE TABLE IF NOT EXISTS bucket_raw (
    hour                TEXT    NOT NULL,
    site                TEXT    NOT NULL, -- 归一化 host（小写、无 scheme/端口/www）
    app                 TEXT    NOT NULL, -- app 标识（claude/codex/...，形状校验不枚举）
    source              TEXT    NOT NULL, -- 客户端每日轮换的随机 id（32 hex）
    asn                 INTEGER NOT NULL DEFAULT 0, -- 上传网络的 ASN（0 = 未知）
    ua_trusted          INTEGER NOT NULL DEFAULT 0, -- 1 = User-Agent 是 LoongPort/ 客户端
    samples             INTEGER NOT NULL, -- 请求数
    errors              INTEGER NOT NULL, -- 失败请求数
    ttft_bins           TEXT    NOT NULL, -- TTFT 直方图计数（JSON 数组，桶边界见 src/bins.ts）
    ttft_count          INTEGER NOT NULL, -- 有 first_token_ms 的样本数（= sum(ttft_bins)）
    input_tokens        INTEGER NOT NULL,
    output_tokens       INTEGER NOT NULL,
    cache_read_tokens   INTEGER NOT NULL,
    cache_creation_tokens INTEGER NOT NULL,
    cost_usd_micros     INTEGER NOT NULL, -- 该桶总花费（微美元）
    breaker_trips       INTEGER NOT NULL DEFAULT 0, -- P4b：非致命熔断跳闸次数（站点侧信号）
    PRIMARY KEY (hour, site, app, source)
) WITHOUT ROWID;

-- 按 IP 哈希的上传限流计数（每小时窗）。不用 KV 存这个：KV 免费档每天只有 1k 写，
-- 限流计数会把它打爆；D1 的写额度是十万行/天。行保留 2 天，cron 清理。
-- ⚠️ 只存 IP 的 SHA-256，不存 IP 本身（接收端不记 IP 是 stats.rs 隐私评审定的义务）。
CREATE TABLE IF NOT EXISTS upload_ip_hour (
    ip_hash TEXT    NOT NULL,
    hour    TEXT    NOT NULL,
    count   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (ip_hash, hour)
) WITHOUT ROWID;

-- P4 模型维度原始桶（v2 客户端）。一行 = 某来源在某小时对某站点某 app 某
-- 模型的聚合。与 bucket_raw 同款幂等（PK + INSERT OR REPLACE）与反作弊列；
-- 站点级聚合仍走 bucket_raw（顶层字段是全量口径），本表只喂模型维度的趋势/分布。
CREATE TABLE IF NOT EXISTS bucket_model_raw (
    hour                TEXT    NOT NULL,
    site                TEXT    NOT NULL,
    app                 TEXT    NOT NULL,
    model               TEXT    NOT NULL,
    source              TEXT    NOT NULL,
    asn                 INTEGER NOT NULL DEFAULT 0,
    ua_trusted          INTEGER NOT NULL DEFAULT 0,
    samples             INTEGER NOT NULL,
    errors              INTEGER NOT NULL,
    ttft_bins           TEXT    NOT NULL,
    tps_bins            TEXT    NOT NULL,
    input_tokens        INTEGER NOT NULL,
    output_tokens       INTEGER NOT NULL,
    cache_read_tokens   INTEGER NOT NULL,
    cache_creation_tokens INTEGER NOT NULL,
    cost_usd_micros     INTEGER NOT NULL,
    anomalies           INTEGER NOT NULL DEFAULT 0, -- P5：被动观察到的模型真伪异常（Anomaly 级）次数
    PRIMARY KEY (hour, site, app, model, source)
) WITHOUT ROWID;

-- 匿名使用统计（relay::stats）的启动上报，ping.ts 写入。一行 = 一个安装。
-- install_id 是客户端在用户**同意告知那一刻**生成的随机 UUID v4（模块专属，
-- 与 device_id / crowd 的日轮换 source 永不交叉）—— 持久 id + 诚实披露是
-- stats.rs 隐私评审定的口径，安装量去重正需要它跨次稳定。
-- 每次 upsert：版本/OS/站点列表刷新为最新，first_seen 保留首次时间；
-- last_seen 是保留期判定键（180 天未见活动即清理，见 index.ts）。
-- ⚠️ 只进 D1、无公开读端点 —— 维护者经 wrangler/dashboard 查询，不进任何公开快照。
CREATE TABLE IF NOT EXISTS stats_installs (
    install_id           TEXT    NOT NULL PRIMARY KEY, -- 随机 UUID v4（客户端同意时生成）
    app_version          TEXT    NOT NULL,
    os                   TEXT    NOT NULL,             -- macos/windows/linux/other（不带版本号）
    site_hosts           TEXT    NOT NULL,             -- 归一化注册域的 JSON 数组（服务端排序去重）
    relay_account_count  INTEGER NOT NULL,             -- 账号行数（非站点数，口径见 stats.rs）
    first_seen           INTEGER NOT NULL,             -- epoch 秒，首次上报
    last_seen            INTEGER NOT NULL              -- epoch 秒，最近上报（保留期判定键）
) WITHOUT ROWID;
