# 待办 / 技术债清单

按「what / why / how-to-repay」三要素记。**已知情、有计划**的占位记在这里；
凭空推后能做的事不算（见全局规则的 defer 准入闸）。

---

## 低余额的**系统通知**（2026-08-04 记，维护者定了「后面要做」）

**what**：余额低于 $5 时只在**应用内**提醒（`OperatorRow` 里那个琥珀色叹号）。
**还没有桌面系统通知** —— 用户不打开 app 就看不到。

**why 现在不做**：不是做不了，是**这一块的产品语义还没定**。它不是「加个 API 调用」，
而要先答三个问题，答错了会变成骚扰而不是提醒：

1. **什么时机触发**？余额每次刷新都判？那用户挂着 app 一天能收到几十条。
2. **怎么去重**？同一个账号一天最多一条？还是「跌破阈值那一次」才发（需要记住
   上一次的余额，而那是个新的持久化状态）？
3. **余额回升后怎么重新武装**？充值到 $50 又跌回 $4，该再发一次 —— 那意味着要存
   「上次通知时的状态」，不只是一个布尔标记。

⚠️ 维护者 2026-08-04 明确答的是「可以先不做，补在 todo 里面，**后面要做**」——
所以这不是「永远不做」，是等他定上面那三条。

**how-to-repay**（前置条件链）：

1. 定上面三个语义（**需要维护者决策**，客户端这边定不了）
2. 装 `tauri-plugin-notification`（`src-tauri/Cargo.toml` + `src-tauri/capabilities/`
   加权限，macOS 首次调用会弹系统授权框 —— 那个体验也要维护者过一眼）
3. 触发点在余额刷新那条链路上（`src/components/operator/OperatorSection.tsx` 里
   拉 `balances` 的地方），判据**复用** `components/operator/lowBalance.ts` 的
   `isLowBalance` —— 别再写一份阈值比较（那会变成两个可能不同步的真相源）
4. 去重状态要落库还是只在内存（app 重启后重新提醒可以接受吗）—— 跟着第 1 步的答案定

**⚠️ 作用域与应用内提醒一致**：只对中转站（operator）行，不对官网直连（vendor）行。
理由见 `tests/lib/lowBalanceScopeContract.test.ts` 的文档（两侧余额币种与类型都不同）。

---

## 匿名统计的**接收端还没建**（2026-08-04 记，`stats.rs` 一直指着这条却没人写下来）

**what**：`src-tauri/src/operator/stats.rs` 的 `ENDPOINT` 还是占位
`https://stats.invalid/v1/ping`（`.invalid` 是 RFC 2606 保留 TLD，万一判断失灵也
打不到真实主机）。客户端这一侧**已经完整可用**：载荷、假名化、失败静默、首启告知
弹窗、设置里的开关都在，只差一个接收端。

`is_configured()` 判占位，所以现在整条链路是 no-op；**首启告知那一屏也不弹了**
（`StatsNoticeDialog` 2026-08-04 加的前置条件）—— 端点没配时同意与不同意的后果
完全相同，问了是消耗用户的信任却换不到数据。

⚠️ **别因为「现在不发也不弹」就删掉这些代码或文案**，端点建好后立刻要用。

**why 现在不做**：卡在**维护者要做的外部动作**（买/配一个接收服务），属 defer 准入闸
时间维度第 (1) 类，不是「花成本所以推后」。

**how-to-repay**（前置条件链）：

1. 定接收端形态（Cloudflare Worker + D1 最省事，与 `config.loongport.dev` 那套
   同一个账号；也可以是任何能收 POST 的东西）
2. **接收端侧的两条义务**（客户端代码管不到，但直接决定这件事是否站得住）：
   **不记来源 IP**、**设保留期**。客户端做再多假名化，服务端记了 IP 就全白费 ——
   见 `stats.rs`「准确的词是假名化而不是匿名」那一节
3. 改 `stats.rs` 的 `ENDPOINT` **一行**（`is_configured` 随之为 true，
   上报与告知弹窗一起自动放行，不需要动别的地方）
4. 那一节还列了三条可选的缓解措施（稀有 host 归 `other` / 每 host 单独 HMAC /
   站点数分桶），都要牺牲数据质量，**按「到底想答什么问题」权衡** —— 不是必做项

---

## 官网直连没有「换一把 key」的入口（2026-08-04 记，codex review 抓出）

**what**：`vendor_provision` 只在本地 `api_key` **为空**时才联网建 key（第 2 步
「本地已有明文 ⇒ 零请求」，那是刻意的正常路径）。于是用户在 DeepSeek 官网手动
删除 / 撤销了那把 key 之后，行内那个「刷新密钥」按钮**是无效的** —— 它把同一把
已失效的 sk 又写进六个平台。目前唯一的出路是**删掉这一行再重新添加**。

**why 现在不做**：需要先定「怎么判断一把 key 还有效」，而这一步**要发网络请求**：

1. 拉一次 `list_keys` 比对本地这把还在不在？—— 但列表只给脱敏值
   （见 memory `deepseek-platform-api-contract`：明文与脱敏值同长 35，只能靠星号
   区分），**比不出是不是同一把**。
2. 拿本地 sk 真去打一次 DeepSeek 的推理端点？—— 那要花钱（哪怕 1 token），
   而且失败原因分不清「key 撤销了」还是「余额不足 / 网络不通」。
3. 无条件删了重建？—— 会把**别的机器**正在用的同一把 key 也废掉
   （见 `vendor/provision.rs` 的「按账号而不按机器」：三台 Mac 共用同一把）。

三条都有实质问题，选哪条是产品决策（愿不愿意为探活花钱 / 愿不愿意影响其它机器），
不是实现细节。**这不是「花成本所以推后」**（那不构成 defer 理由）—— 是判据本身
依赖一个还没有的服务端能力：DeepSeek 的 key 列表不给可比对的标识。

**how-to-repay**（前置条件链）：

1. 定语义：撞到「本地有 key 但服务端已撤销」时，是让用户显式点「换一把新 key」
   （承担影响其它机器的后果），还是自动探活（承担成本与误判）**需要维护者决策**
2. 若走「显式换一把」：加 `vendor_reset_key` 命令 —— 清 `loongport_vendor.api_key`
   后复用 `provision_impl`（它第 2 步会因为空值而走建新的那条路，**不必新写建 key 逻辑**）
3. 前端在 `src/components/operator/VendorRow.tsx` 加入口，文案要说清「会让其它机器
   上的这把 key 失效」
4. 若走「自动探活」：先确认 DeepSeek 有没有便宜的 key 校验端点
   （DeepSeek 是闭源的，拿不到源码当契约 —— 只能实测）

**当前的诚实做法**（已落地）：`VendorRow.tsx` 那个按钮的注释写清了它**不会**换 key，
以及撞上撤销时该怎么办 —— 不让读代码的人以为那条路已经覆盖了。

---

## Node 钉在 22.12.0，已旧 11 个 minor（2026-08-04 记）

**what**：`.node-version` 是 `22.12.0`，而 Node 22 已发布到 `22.23.2`。三个 workflow
现在都跟着这个文件走（2026-08-04 统一的），所以升它是一处改动、三处生效。

**why 现在不做**：**不是做不了，是这一轮不该顺手做**。当前这轮在收尾首个公开发布，
升 Node 会同时影响 CI、出正式包的那条腿、以及贡献者的本机环境 —— 撞出问题会把发布
一起卡住。已经踩过一次：把 Node 从写死的 20 改成跟文件走（22.12.0）之后，Windows
ARM64 那格在 `corepack prepare` 验签处炸了（`Cannot find matching keyid`，Node 自带
的 corepack 里烧死的 npm 公钥过期），得单独修一轮。

**how-to-repay**（前置条件链）：

1. 选定目标版本（22.x 最新的 LTS patch），改 `.node-version` **一处**
2. 顺手验证能否**删掉** `release.yml` 里 `Setup pnpm (Windows ARM64)` 那步的
   `npm install -g corepack@0.35.0` —— 如果新 Node 自带的 corepack 已带新公钥，
   那一行就是多余的（不确定哪个版本开始带，只能实测）
3. 跑一次 tag 构建验证三个平台（**不能只跑 CI**：ARM64 那格只在 release.yml 里）
4. `CONTRIBUTING.md` 写的是「Node 22（见 `.node-version`）」，指向文件而非具体号码，
   ⇒ 升版本不用改文档

---

## dependabot 的 25 条告警：分三类，只有一类要紧（2026-08-04 记）

仓库转公开、开了 dependabot alerts 之后一次性冒出 25 条（2 critical / 10 high /
12 medium / 3 low）。**分类核过一遍，别被 critical 那个词带着走**：

**（1）npm 那 21 条全是 `development` scope** —— vite / vitest / rollup / postcss /
esbuild / ws / form-data / picomatch / @babel/core。两个 critical 都是 vitest，且是
**Vitest UI server** 的漏洞（监听端口时可任意读文件），而本仓 CI 与本机都只跑
`vitest run`（不带 UI、不监听）。这批不进产物，但**该升** —— 它们是每天在用的工具链，
dependabot 已自动开 PR，跟着合就行（`pull_request` 触发 2026-08-04 恢复了，PR 有 CI 验）。

**（2）Rust 里有 6 条指向压根没人依赖的包**：`openssl`（8 条中的大部分）与
`quinn-proto`。`cargo tree -i openssl` / `-i quinn-proto` 都打印 "nothing to print"
—— 它们是 `Cargo.lock` 里的陈旧条目，dependabot 从 lock 文件读所以报了，实际编不进
产物。**升它们没有实际收益**（但也无害，dependabot 的 PR 合了能让告警清零、省得每次
看到 Security 页上一堆红字）。

**（3）真在依赖树里的 Rust 包**：`aws-lc-sys` ×5、`rustls-webpki` ×4、`tar` ×3、
`tauri`、`serde_with`、`glib`、`rand`。这批是 reqwest / rustls / tauri 的传递依赖，
**跟着 dependabot 的 PR 升**（PR #10~#15 已开）。

**⚠️ 顺带发现的一笔独立债：`tauri-plugin-updater` 还在依赖里且代码在用。**
`Cargo.toml:37` 声明它、`commands/settings.rs:4` 引 `UpdaterExt`、`capabilities/
default.json` 也给了权限 —— **但 `tauri.conf.json` 的 `plugins` 只有 `deep-link`**，
插件没注册 ⇒ 那条「检查更新」链路运行时必然失败。它也是上面 rustls / aws-lc 那批
漏洞的引入路径之一。

**how-to-repay**：定「要不要自己的更新渠道」。要 → 配 endpoints + 换自己的 pubkey +
注册插件（三件缺一不可，`release.yml` 的 `Prepare Tauri signing key` 那步注释里写了）；
不要 → 把依赖、`UpdaterExt` 那条链路、capabilities 权限一起删干净，别留「声明了但
不工作」的中间态。**需要维护者决策**（产品问题：靠 GitHub Releases 手动更新够不够）。

---

## 数据库仍是 rollback-journal 模式，而现在有第二个进程会读它

**what**：`database/mod.rs` 建连接时设了 `foreign_keys` 与 `auto_vacuum`，但**没设
`journal_mode = WAL`**。rollback 模式下写者持 EXCLUSIVE 锁会**直接阻塞读者**；
WAL 模式下读写可并行。

**why 现在才成为问题**：生图 MCP（`operator/imagegen_mcp.rs`，2026-08-05 加）是
**第一份从第二个进程读这个库**的代码。它已经做对了两件事 —— 以 `SQLITE_OPEN_READ_ONLY`
打开（绝不拿写锁）、依赖 rusqlite 默认的 5s `busy_timeout`。但主程序一次**长写**
（迁移 / 备份 / 启动时的 `incremental_vacuum`）仍可能让它等超 5s ⇒ MCP 启动报
「打开数据库失败」⇒ 宿主那侧看到的是「生图工具起不来」。

概率低（要正好撞上那几秒），且**属既存设计**而非本次引入，所以按 defer 准入闸记在这里
而不是顺手改 journal 模式 —— 那是全库行为的变更，值得单独一轮验证（WAL 会多出
`-wal` / `-shm` 两个文件，备份与「换数据目录」两条路径都要重新验）。

**how-to-repay**：在 `database/mod.rs` 的连接初始化里加 `PRAGMA journal_mode = WAL`，
然后复核三处：① `database/backup.rs` 的备份是否仍完整（WAL 下要 checkpoint 或用
sqlite 的备份 API，直接拷主文件会丢最近的写）；② `app_store` 换数据目录那条路径；
③ Windows 上多进程访问同一个 WAL 库的行为。
