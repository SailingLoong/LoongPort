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
