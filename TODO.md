# 待办 / 技术债清单

按「what / why / how-to-repay」三要素记。**已知情、有计划**的占位记在这里；
凭空推后能做的事不算（见全局规则的 defer 准入闸）。

---

## 一键从 cc-switch 同步配置与数据（2026-08-05 记，2026-08-07 **导入时合并已落地**）

> **已完成（2026-08-07，`feat/cc-switch-import`）**：`relay/cc_switch_import.rs` 实现
> 覆盖式导入（复用 `import_sql_string_preserving`：备份 + 原子替换 + 迁移 + 版本校验），
> 三个入口（设置页按钮 / LoongPort 图标旁按钮 / 首启弹窗）共用
> `CcSwitchImportDialog`。源库只读打开、绝不动 cc-switch 那边。**导入时**的冲突归属
> （同指纹 `域名 + sk` ⇒ 托管侧胜、不导入、报告列出）已做，集成测试钉着源库字节不变 +
> 「已手动维护」判定不变。
>
> **剩下的一半是「provision 时收编」**（direction 2，维护者定了下轮做）：导入**后**新加
> 中转站、provision 建出的新档位与**已导入的 cc-switch 条目**同指纹时，把那条 cc-switch
> 记录收编（删 + 报告）。实现要点见下方「冲突归属规则」第二段；本轮有意不做 ——
> provision 生成的 key 是 `LoongPort/<账号>/<平台>/<分组>` 命名，与用户手工配的 sk 撞上
> 概率低，且要动 relay/vendor 的 provision 热路径。

**what**：cc-switch 老用户装上 LoongPort 后看到的是**全空的**，得把 provider、MCP、
skills、prompt 一件件重新配。两边的数据目录完全隔离：

| | 目录 | 数据库 |
|---|---|---|
| cc-switch | `~/.cc-switch/` | `cc-switch.db` |
| LoongPort | `~/.loongport/`（`config.rs:21` `APP_DIR_NAME`） | `loongport.db`（`config.rs:24`） |

⚠️ **这个隔离是有意的、别改**：两个 app 可以同时装、同时跑，共用一个库会互相踩
（`app_config.rs` 那些 `~/.cc-switch` 引用是**旧数据识别**用的兜底，属 CLAUDE.md
§三点五「它本身就是兜底」那一类，绝不能改）。要的是**一次性拷过来**，不是共用。

**要做的**：设置页给一个「从 cc-switch 导入」按钮 —— 检测到 `~/.cc-switch/cc-switch.db`
存在就显示，点了把配置与数据搬过来。

**why 现在才记**：本轮在收尾首个公开发布，之前的假设是「新用户为主」。但 LoongPort 是
cc-switch 的 fork，**实际盘子里很大一部分人本来就是 cc-switch 用户** —— 让他们手工重配
一遍是最劝退的一步，且这一步发生在他们对产品还没有任何信任的时候。

**how-to-repay**（前置条件链）：

1. **先定范围：哪些表该搬、哪些不该。**（**需要维护者决策**，这是本项唯一的真问题）
   - 该搬：provider 配置、MCP、skills、prompt —— 用户自己攒的东西。
   - **不该搬**：LoongPort 自己的表（`loongport_*` / relay 凭据 / `device-id`），
     cc-switch 里压根没有。
   - **要单独想**：cc-switch 里那些**指向别家中转站**的 provider。照搬进来会与
     LoongPort 托管的档位混在一列里，而两者语义不同（见 CLAUDE.md「§什么时候可以不复用」
     里 `ProviderCard` vs 托管项那段）。是全搬、只搬官方直连、还是让用户勾选？
2. **复用 `import_config_from_file`**（`commands/import_export.rs:42`）**别新写一套** ——
   它已有 `db.import_sql` + `run_post_import_sync` + 自动建备份（返回 `backup_id`）
   三件事。新命令做的应该只是「把 `~/.cc-switch/cc-switch.db` 导成 SQL 再喂给它」，
   或直接走 `db.import_sql` 的同一条路。
3. **导入前必须先备份自己的库** —— 复用第 2 步那条路径自带的备份，
   失败要能 `restore_db_backup`（`import_export.rs:152`，已有）。
4. 前端入口跟着上游的形状放（`DeepLinkImportDialog.tsx` 是同类交互的现成参照）。
   文案要说清「这会把 cc-switch 的配置**复制**过来，不动 cc-switch 那边」。
5. ⚠️ **schema 会分叉**：两边的 `cc-switch.db` / `loongport.db` 迁移版本可能不同步
   （LoongPort 加了自己的表和迁移）。导入时**必须校验源库版本**，比自己旧就先跑迁移、
   比自己新就拒绝并说明原因 —— 不能直接灌进去。这条是实现上最容易出错的地方。

### 冲突归属规则（2026-08-05 维护者定的，本项的核心语义）

**规则**：同一把 key 若既存在于导入进来的 cc-switch 配置里、又属于 LoongPort 管的
中转站 / 官网直连模块，则**归 LoongPort 那一侧维护**（中转站 / DeepSeek 官网模块），
不留成两条并存的记录。两个方向都适用：

- **导入时**：cc-switch 那条与已有托管项撞了 ⇒ 托管项胜，那条不导入。✅ **2026-08-07 已落地**
  （`relay/cc_switch_import.rs`，指纹判据见下）。
- **导入后新加中转站**（注册 / 登录 / provision）：新建的 key 与已导入的 cc-switch 条目
   撞了 ⇒ 转由中转站模块维护，把那条 cc-switch 记录收编掉。⏳ **下轮做** —— 挂点：
   relay provision 的 `save_provider`（`commands/relay.rs:1008`）与 vendor 的
   （`commands/vendor.rs:543`）写档位之前，扫同 `(origin, sk)` 指纹的**非托管**条目收编之。

**为什么不能并存**：两条指向同一个上游的记录，用户看到的是重复档位，
而其中一条不受 provision 管（改模型 / 换 sk 都不会跟着动）⇒ 用哪条完全看运气。
这与 CLAUDE.md §三点六「同一事实散在多处」是同一类病。

**判据（维护者给的）**：`域名 + sk 密钥` 两者合起来做唯一键 —— 单看 sk 可能撞
（不同站点的 key 格式相同），单看域名会把同站点的多个档位误并成一个，合起来不会重复。

⚠️ **但这个键不能直接当持久化主键，因为 sk 会变**（`provision` 会「只换 sk」——
见 `provision.rs:607` / `:830-838` 那两处文档）。换一次 sk，同一个档位的键就变了。
所以正确用法是**分两层**：

| 用途 | 用什么 |
|---|---|
| **持久身份**（provider_id、DB 主键） | 保持现状：`SHA256(site_origin/account_id/group_id)`（`provision.rs:362`）、vendor 侧 `provider_id_for(vendor_id, account_id)`（`vendor/provision.rs:40`）。**换 sk 不变**，这是它比 sk 更适合当身份的原因 |
| **冲突检测**（判「这条 cc-switch 记录是不是同一个东西」） | 维护者给的 `域名 + sk` —— 只在导入 / provision 那一刻比一次，比完就按上表的持久 id 落库 |

即：**`域名 + sk` 是识别用的指纹，不是身份。** 别拿它建唯一索引，否则每次
provision 换 sk 都会变成「插入一条新的」而不是「更新已有的那条」。

**⚠️ 取域名与 sk 本身就是个活儿，别假设有现成字段**：cc-switch 的 provider
**没有 `base_url` / `api_key` 列** —— 全塞在 `settings_config`（`provider.rs:15`，
一个 `serde_json::Value`）里，且**每种 app 的形状都不同**（Codex 是 TOML 片段、
Claude 是 JSON、Gemini 又一套）。所以：

- **取 base_url 复用上游的 adapter trait**：`proxy/providers/adapter.rs:21` 的
  `extract_base_url(&Provider)`，各 app 已各自实现（`codex.rs:672` / `claude.rs:703` /
  `gemini.rs:165`）。别自己解析 `settings_config`，那等于把上游三套解析逻辑重写一遍。
- **取 sk 复用我们已有的** `provision.rs:1028` 的
  `extract_api_key(settings_config, app_type)`。
- 拿到 base_url 后**要归一化到 origin 再比**（去掉 path、统一大小写、剥末尾斜杠与
  默认端口）：cc-switch 那侧是 `https://bestapi.store/v1` 这种带 path 的，
  托管项那侧是 `site_origin`（`https://bestapi.store`），不归一化会全部漏检。

**被收编的那条要怎么处理**（**需要维护者决策**，第 1 步那个「别家中转站 provider」
的范围问题会先决定这里）：直接删、还是保留但标记「已由 LoongPort 接管」？
删了用户在 cc-switch 里的自定义（改过的模型名 / 别名）就没了；留着又回到「两条并存」。
倾向是**删并在导入报告里列出「这 N 条已由 LoongPort 接管」**，让用户知道去哪找它们。

---

## 低余额的**系统通知**（2026-08-04 记，维护者定了「后面要做」）

**what**：余额低于 $5 时只在**应用内**提醒（`RelayRow` 里那个琥珀色叹号）。
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
3. 触发点在余额刷新那条链路上（`src/components/relay/RelaySection.tsx` 里
   拉 `balances` 的地方），判据**复用** `components/relay/lowBalance.ts` 的
   `isLowBalance` —— 别再写一份阈值比较（那会变成两个可能不同步的真相源）
4. 去重状态要落库还是只在内存（app 重启后重新提醒可以接受吗）—— 跟着第 1 步的答案定

**⚠️ 作用域与应用内提醒一致**：只对中转站（relay）行，不对官网直连（vendor）行。
理由见 `tests/lib/lowBalanceScopeContract.test.ts` 的文档（两侧余额币种与类型都不同）。

---

## 匿名统计的**接收端还没建**（2026-08-04 记，`stats.rs` 一直指着这条却没人写下来）

**what**：`src-tauri/src/relay/stats.rs` 的 `ENDPOINT` 还是占位
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
3. 前端在 `src/components/relay/VendorRow.tsx` 加入口，文案要说清「会让其它机器
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

## 剩下 2 条 dependabot 告警：被上游 semver 卡住，不是没修（2026-08-05 记）

2026-08-04 转公开时一次性冒出 25 条，绝大多数已经清掉（dependabot 自己的 PR
#8~#18，加上 2026-08-05 手工顶 `aws-lc-rs` 那次）。**剩下的 2 条不是漏了，是
`cargo update` 拒绝动它们** —— 上游包的 caret 约束把版本钉死了：

| 包 | 现在 | 需要 | 谁钉住它 | 实际暴露面 |
|---|---|---|---|---|
| `glib` 0.18.5 | medium | ≥ 0.20.0 | `webkit2gtk` 2.0.2 要 `^0.18` | **macOS / Windows 编不到它** |
| `rand` 0.7.3 | low | ≥ 0.8.6 | `phf_generator` 0.8.0 要 `^0.7` | **只有 build-dependency 边** |

两条都验证过，不是推测：

```sh
# glib 在两个发布 target 下都是 "nothing to print"（它走 gtk，Linux only）
cargo tree -i glib --target aarch64-apple-darwin
cargo tree -i glib --target x86_64-pc-windows-msvc
# rand 0.7.3 只在 --edges build 时出现，normal 边为空（走 phf_codegen 的编译期代码生成）
cargo tree -i rand@0.7.3 --edges normal --target all
```

⇒ 两条**都不进用户拿到的产物**：`glib` 那条要等我们真出 Linux 包才有意义
（「支持范围」表里 Linux 还在「在做」），`rand` 那条只在编译我们自己的机器上跑。

**how-to-repay**（都不是我们能推的，等上游）：

- `glib` → 等 `webkit2gtk` crate 放宽到 `^0.20`。它由 `tauri` 拉进来，所以实际是等
  Tauri 那条链升 gtk 生态 —— **跟着 tauri 的版本走即可，别自己 patch**。
- `rand` → 等 `kuchikiki` / `selectors` 升 `phf` 到 0.11+（`tauri-utils` 的依赖）。同上。
- **判断「是否可以动了」的办法**：`cargo update -p glib --precise <版本> --dry-run`，
  它会把拦住的那条约束链整条打印出来，比翻上游 issue 快。

⚠️ **别为这两条改 `Cargo.toml` 加 `[patch]`** —— 收益是「Security 页少两行红字」，
代价是我们自己维护一条 fork 的依赖线、且下次 tauri 升级时冲突。上面已经算清了
它们编不进产物，写在这里就是为了让下一个人不用重算一遍。

---

## 数据库仍是 rollback-journal 模式，而现在有第二个进程会读它

**what**：`database/mod.rs` 建连接时设了 `foreign_keys` 与 `auto_vacuum`，但**没设
`journal_mode = WAL`**。rollback 模式下写者持 EXCLUSIVE 锁会**直接阻塞读者**；
WAL 模式下读写可并行。

**why 现在才成为问题**：生图 MCP（`relay/imagegen_mcp.rs`，2026-08-05 加）是
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

---

## 连通检测的探测结论是硬编码中文，没走 i18n

**what**：`services/stream_check.rs` 的 `probe_models` 直接返回中文串
（`"密钥已失效（401）"` / `"只能生图（…），不能对话"` / `"N 个模型（…）"` 等 5 条），
原样显示在 toast 里 ⇒ en / ja / zh-TW 用户看到中文。

**why 当时没做**：这个功能其余部分的用户可见文案都过了 i18n（7 个 key × 4 语言齐全），
所以这是个真的不一致，不是有意为之。但它是**诊断按钮的附加信息**（description 那一行），
非中文用户看到中文仍然读得懂关键部分（`401` / `gpt-image-2` 这些是符号），
且不影响任何判定 —— 按尺子2 不值得为它在发版前引入一层 enum + 前端格式化。

顺带：这一层的既有后端文案是英文的（`"Reachable"` / `"Check failed"`），
所以现在两个方向都不一致。

**how-to-repay**：`probe_models` 改为返回一个小 enum + 模型列表
（如 `ProbeVerdict::KeyExpired` / `ImageOnly(Vec<String>)` / `Models { total, head }`），
在 `useStreamCheck.ts` 里用 `t()` 格式化。四个语言文件各加 4-5 个 key。
`StreamCheckResult.model_used` 那个 TEXT 列要么存 enum 的判别式 + JSON、
要么另加一列 —— 注意它同时是 `stream_check_logs` 的历史数据，改格式要考虑旧行怎么读。

---

## HSTS 的 max_age 还是观察期的 1 天（2026-08-05 记）

**what**：loongport.dev 的 HSTS 于 2026-08-05 开启，但 `max-age=86400`（1 天）——
那是**渐进上线的观察期值**，不是最终配置。业界标准是 1 年（`31536000`）。

**why 当时不直接上 1 年**：HSTS 是那类**难以撤销**的设置 —— 浏览器会记住 max_age，
期间即使在 Cloudflare 关掉，已访问过的用户浏览器仍拒绝走 HTTP。用 1 天起步意味着
万一撞上意外（例如将来某个子域没证书），等一天就能恢复而不是等一年。

**why 现在还没调**：需要先观察一周确认无异常 —— 而「无异常」的判据是时间，
不是能立刻做完的事（defer 准入闸时间维度第 2 类）。

**how-to-repay**（前置条件链）：

1. 2026-08-12 之后确认这一周没有 HTTPS 相关的访问异常
2. Cloudflare → loongport.dev → SSL/TLS → Edge Certificates → HSTS，
   把 max_age 改成 `31536000`（1 年）。**或用 API**（token 有 Zone Settings:Edit）：
   `PATCH /zones/{zone}/settings/security_header`，
   `{"value":{"strict_transport_security":{"enabled":true,"max_age":31536000,...}}}`
3. `include_subdomains` 与 `preload` **仍建议保持 false**：
   - 前者会把 HSTS 强加到所有子域，而 `config.loongport.dev` 那类将来可能另做安排
   - 后者一旦进了浏览器内置的 preload 列表，**移除要等数月**，对一个还在演进的
     项目不值得
4. 验证：`curl -sI https://loongport.dev/ | grep -i strict-transport`
   应看到 `max-age=31536000`

**⚠️ 别把这条当成「可以不做」**：停在 1 天不算错（安全收益已经拿到大部分），
但那意味着每个用户的保护每天过期一次。要么调上去，要么明确决定就停在这儿并删掉本条。
---

## 档位配置（尤其模型映射）应由远端配置文件下发（2026-08-05 记，维护者定了「将来要做」）

**what**：每个档位的默认配置现在是**编译期 Rust 字面量** —— 官网直连那份在
`vendor/deepseek.rs` 的 `config_for`（六个平台的 `(base_url, model)`），Claude 的模型
映射（opus/fable → pro、sonnet/haiku/subagent → flash）也在同一处。
⇒ **模型改名或新增档位，只能靠发版**。

**why 现在这样**：远端配置那套（`relay/remote_config.rs`）已经跑着了，
但它当前的 schema 只有三个键（`sponsors` / `affCodes` / `promoCodes`），
不含档位配置。本轮改的是模型映射的**取值**，把整套配置搬去远端是另一件事，
按尺子2 不塞进这次。

**为什么值得做**（不是投机预留）：厂商改模型名这件事**已经在发生** ——
`deepseek-v4-pro` / `-flash` 这两个名字本身就是上游 preset 跟着 DeepSeek 改过来的。
每次改名都要求用户升级客户端，而配置下发本就是 `remote_config` 存在的理由。

**how-to-repay**：

1. `RemoteConfig` 加一个键（如 `tierConfigs`），**必须带 `#[serde(default)]`** ——
   同 `promo_codes` 那条注释的双向兼容理由（旧客户端读新配置 / 新客户端读旧配置）。
2. 取值处改成「远端有就用远端、否则回落到内置字面量」：
   `vendor/deepseek.rs::config_for` 与 Claude 模型映射那处。
   **内置那份要留着**（首次启动、离线、验签失败都得能工作 —— 三层回落是
   `remote_config` 已有的设计，别绕过它）。
3. 配置源文件与签名脚本在档案仓 `remote-config/`，改 schema 要同步那份 + 重新签名。
4. ⚠️ **与「已手工维护」的判据有交互**：`is_user_edited` 是「跟默认值比对」，
   而默认值一旦能远端变更，同一份配置可能今天算「默认」、明天算「已改过」。
   `candidate_models` 现在靠「当前默认 + 全部历史默认值」链式比对来兜
   `DEFAULT_MODEL` 变更那天的误报 —— 远端下发后这条链要能容纳远端给过的历史值，
   否则用户会看到档位集体误报「已手工维护」。**这是本项真正的难点，别当成加个字段。**

---

## `is_user_edited` 不覆盖 hermes / openclaw / opencode（2026-08-05 记，加 vendor 编辑功能时暴露）

**what**：`relay/provision.rs` 的 `api_key_location` 只认 codex / codex-image /
claude / claude-desktop / gemini。剩下三个平台落到 `_ => None` ⇒ 对它们：

| 受影响的能力 | 症状 |
|---|---|
| 「已手动维护」标记 | `is_user_edited` 恒为 `None` ⇒ **界面上永远不显示标记**，即使用户改过 |
| 「恢复默认配置」 | `extract_api_key` 读不出 sk ⇒ 命令直接报「读不出密钥」 |
| provision 的「只换 sk」 | `patch_api_key` 失败 ⇒ **回落到全量重写**，把用户的编辑整份冲掉 |

第三条最糟：它不是「功能缺失」而是**静默的数据丢失**，且发生在用户点「获取密钥」
（他以为只是刷新一下）的时候。

⚠️ **2026-08-05 顺手修了 claude-desktop 那个**（它与 claude 同形、加一行就够）。
剩下三个是结构问题，见下。

**why 现在不做**：`api_key_location` 返回 `(section, field)` **两段**，而这三个平台的
sk 位置表达不了：

| 平台 | sk 在哪 | 出处 |
|---|---|---|
| hermes | **顶层** `api_key` | `deeplink/provider.rs:604` |
| openclaw | **顶层** `apiKey` | 同上 `:563`（`build_additive_app_settings`） |
| opencode | `options.apiKey`（两层，且 `options` 嵌在 provider 名字下） | 同上 `:534` |

补它要把那个返回类型改成能表达「顶层」与「多层路径」的形状（如 `&[&str]` 路径），
**连带动 `patch_api_key` / `extract_api_key` 的签名与 relay 侧全部调用方** ——
属「借清债名义翻修无关模块」，不在加 vendor 编辑功能这一轮的手伸到的范围内。

**how-to-repay**：

1. `api_key_location` 改成返回字段路径（`Option<&'static [&'static str]>`），
   codex 那条变 `["auth", "OPENAI_API_KEY"]`、hermes 变 `["api_key"]`、
   opencode 变 `["options", "apiKey"]`（⚠️ opencode 的 provider 名字是动态的，
   得先确认那一层的键怎么定 —— 看 `build_opencode_settings` 的 `json!` 结构）。
2. `patch_api_key` / `extract_api_key` 跟着走路径而不是两段。
3. **有一条测试正等着这个修完**：`vendor::provision::tests::`
   `user_edited_is_currently_undecidable_for_three_platforms` 钉的是**当前**行为
   （那三个平台返回 `None`）。补完之后它会红 —— 那时把断言改成 `Some(false)`，
   **别当成回归**（测试文档里也写了这句）。
