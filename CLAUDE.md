# LoongPort 代码仓规则

产品说明见 [LOONGPORT.md](LOONGPORT.md)（那份给人看：它替用户做什么、六条硬约束、怎么打包）。
本文件只讲**写这个仓的代码时怎么决策**。

## 设计档案在另一个仓

设计文档、进度、spec 不在本仓，在同级的档案仓里（需要时用 `/add-dir` 单次挂载，别常驻）。
维护者本机的具体布局见工作区那份 `CLAUDE.md`（不入任何仓）。

**上一代实现是「参考不复用」**：它的 operator 层比现在这版复杂一个数量级（云同步边界、
多 app 展开、更多分支裁决），照搬会把这版简化的成果丢掉。查它的**结论**（实测记录、
某个设计为什么那样分）是对的，照抄它的**实现**是错的；那边带行号的引用基于旧的子模块
指针，引用前先 `grep -n` 复核。

## 一、最高优先级：能复用 cc-switch 的就复用

**这个仓是 cc-switch 的 fork，底层要跟着上游升级。** 所以「复用上游」不是风格偏好，
而是**决定未来升级成本的架构约束** —— 每一处自己另写的东西，都是将来 merge 上游时要手工
处理的冲突；每一处复用上游的东西，上游改进了我们免费拿到。

### 判定顺序（自上而下，命中即停）

1. **上游已有的组件 / hook / 工具函数 / 类型** → 直接用。
   例：折叠用 `src/components/ui/collapsible.tsx`（Radix 封装，已在仓里），
   不要引第三方折叠库、也不要自己写展开动画。
2. **上游已有的视觉 token**（间距、圆角、选中态、hover 效果）→ 抄它的值。
   判据：新页面和旧页面放一起，看不出是两个人写的。
3. **上游已有的模式**（数据流、命令命名、错误处理形状）→ 照它的形状写。
4. 以上都没有 → 才新建，且**新建的东西尽量收在自己的目录里**
   （`src/components/operator/`、`src-tauri/src/operator/`），别散进上游文件。

### 改上游文件时：改动面越小越好

不得不动上游文件时（如 `App.tsx` 的视图分流、`ProviderList` 加一层过滤），
**只改必须改的那几行**，把逻辑放进自己的新文件里让它调用。

反例：为了实现一个功能把上游某个 600 行组件重构一遍 —— 那等于放弃了那个文件的上游升级。

### 什么时候可以不复用

- 上游那套**语义上不适用**：例 `ProviderCard` 服务的是「用户手工配置的 provider」
  （可编辑、可删除、可拖拽排序），而 LoongPort 的托管项没有这些操作 —— 硬塞进去会让
  两种形态互相污染。这时另建组件是对的，但**视觉 token 仍要抄**。
- 上游的默认行为**对 LoongPort 有害**：例 updater 端点指向 cc-switch 自己的发布源，
  留着会把用户升级成 cc-switch（见 `lib.rs` 里那段说明）。这类要明确禁用并写清理由。

判据一句话：**「不复用」要能说出上游那套具体哪里不适用，说不出就是复用**。

## 二、技术栈事实（别套错工具）

| 项 | 实际 | 常见误判 |
|---|---|---|
| UI 库 | **Radix UI + Tailwind v3**（shadcn/ui 那套） | 不是 Semi Design、不是 antd、不是 MUI |
| Tailwind | **v3.4.x**，配置在 `tailwind.config.cjs`，CSS 用 `@tailwind base` | 不是 v4 —— 别套 `@theme` / `@import "tailwindcss"`，v3 不认，样式会当场崩 |
| 图标 | `lucide-react` | 别引第二个图标库 |
| 样式合并 | `clsx` + `tailwind-merge`（`cn()`） | 别手拼 className 字符串 |
| 后端 | Tauri 2 + Rust，SQLite 走 `rusqlite` | — |

`src/components/ui/` 下是标准 shadcn 封装（`collapsible` / `accordion` / `dialog` /
`select` / `tabs` …，共 23 个），**先翻那个目录再考虑新建**。

### ⚠️ 许可证界线：sub2api 是 LGPL-3.0，本仓是 MIT

| 项目 | 许可证 | 我们能做什么 |
|---|---|---|
| **cc-switch**（本仓 fork 源） | **MIT** | **可自由复用代码** —— §一「能复用就复用」讲的就是它 |
| **sub2api**（对接的运营商后端） | **LGPL-3.0 或更高** | **只能读，不能抄代码进本仓** |

**为什么读它没问题**：我们与 sub2api 的关系是**HTTP 客户端**，不链接、不包含它的代码 ——
跟浏览器访问一个 LGPL 网站一样，不构成衍生作品。

**可以从它源码拿的（接口事实，不受版权保护）**：
端点路径、HTTP 方法、鉴权方式、请求/响应的字段名与类型、状态码语义、
业务规则的**结论**（如「高峰倍率只对订阅型分组生效」）。

**不能拿的（表达形式，受版权保护）**：
整片函数实现、成套的 struct 定义照搬、算法代码逐行翻译。
⚠️ 具体踩点：它前端有个 `platform → app` 映射函数（`KeysView` 的 `ns()`）与我们的
`platform_map` 做同一件事 —— **参考它的取值域可以，照抄那个函数不行**。
我们的 `platform_map` 是独立实现（穷尽 match + 编译期基数闸），有意不同构。

**一句话判据**：**写下来的是「那边的接口长什么样」就没问题，是「那边的代码怎么写的」就不行。**

（顺带：sub2api 的 README_CN 声明「从未授权任何个人或组织基于本项目开展商业化运营」——
那是针对「拿它的代码搭站运营」，不针对「写一个客户端连它」。但商业化前值得再确认。）

### 查 sub2api 的行为：先找它的 Go 源码，别逆推线上 JS

对接 sub2api（端点、字段、鉴权、计费规则）时，**优先看 sub2api 后端的 Go 源码**
（开源，`Wei-Shaw/sub2api`；维护者本机已 clone，路径见工作区那份 CLAUDE.md）。
它是契约本身，比读线上 SPA 的 minified bundle、比历史实测记录都权威一个量级 ——
能给到「哪个 handler 第几行填了哪个字段」级别的证据。

三条纪律（都踩过）：

1. **只认 `routes/*.go` 里的路由注册，handler 上方的注释不可信**：
   `user_handler.go` 注释写 `GET /api/v1/users/me` 而真实路由是 `/user/profile`；
   `api_key_handler.go` 写 `/api/v1/api-keys/:id` 而真实是 `/keys/:id`。
2. **注意版本差**：本地 clone 的 commit 与线上跑的版本常常不同
   （实测：本地 `0.1.165` vs 线上 `0.1.169`）。凭源码下的结论，落地前对线上响应复核一次。
3. **别用 HTTP 状态码探测 SPA 路由**：sub2api 前端是 Vue SPA，
   `/topup` `/recharge` `/wallet` 全返回 200（同一个 `index.html`），
   但路由表里根本没这些路径。要查路由得读打包后的 JS 或源码。

### 读上游源码之前：先查代码地图

档案仓里有一份 cc-switch 的 zread wiki（30 页，覆盖 Tauri 2 架构、AppState、SQLite schema
与迁移、Provider 数据模型、Live Config 写入、路由与故障转移、React 组件架构、i18n、测试
体系等）。**读上游源码前先查它，能省掉一轮 grep。**

三条硬约束（都踩过或实测过）：

1. **别 `@` 导入、别软链进本仓** —— 全量 376k 字符，`@` 进来当场炸上下文；且它 untracked
   在子模块工作树里、不入任何 git，软链 commit 后在新 clone 的机器上是悬空链接。按需读单页。
2. **它正文写的「当前版本 3.18.0」是错的**（抄了旧 README），别引它当事实。
   但**行号是准的** —— 六处抽样全部对齐。
3. **子模块指针一 bump 行号就集体失准**（纯注释 commit 也能让整片下移）。指针不自动 bump
   所以当前稳；bump 那天要么重新生成，要么降级成「只读结构、不引行号」。

## 三、LoongPort 自己的代码在哪

```
src-tauri/src/operator/     ← 运营商链路（api / creds / login / provision / chatgpt_app）
src-tauri/src/commands/operator.rs
src/components/operator/    ← 前端面板
src/lib/api/operator.ts     ← 前端类型与 invoke 封装
```

碰这几处之外的文件时，先问一句「这是在改上游吗、改动面能不能更小」。

### 这是 fork，不是「把 cc-switch 当依赖引入」

16.9 万行 Rust 里我们自己写的只有 **5621 行（3.3%）** —— 上游代码是**躯干**，我们在它身上
加东西。所以别把它想成 `import cc_switch`：没有那层边界，`operator/` 与上游代码在同一个
crate、共享 `AppState`、共用它的 `ProviderService` / `write_live_snapshot` / deeplink 构造。

**这正是 §一「能复用就复用」的物理基础**，也是「改上游文件要改动面最小」的原因。

## 三点五、改「cc-switch」这个名字：判据是**会不会跨出进程边界**

fork 之后到处都是上游的名字。哪些该改、哪些改了会坏，**不看它在底层还是上层，
看那个字符串会不会离开本进程**：

| 类别 | 例子 | 处理 |
|---|---|---|
| **只活在进程内 / 只给人看** | 日志文件名、启动日志、托盘 tooltip、弹窗文案 | ✅ **直接改**，零风险 |
| **跨出去了，但能「读宽写窄」** | `model_provider` 标记（写进用户 `~/.codex/config.toml`）、model catalog 文件名 | ✅ **改，但必须留兜底**，见下 |
| **跨出去且无法兜底** | crate name（决定二进制名，118 处引用，改了收益为零） | ❌ **不改** |
| **它本身就是兜底** | `LEGACY_..._ID = "ccswitch"`、`app_config.rs` 提到的 `~/.cc-switch/config.json` | ❌ **绝不改** —— 改了就认不出旧数据 |

### 「读宽写窄」是改持久化契约的唯一安全姿势

**识别认全部历史值，写入只产出当前值。** 上游自己就用这个模式
（`codex_history_migration.rs` 的 `CC_SWITCH_LEGACY_CODEX_MODEL_PROVIDER_IDS`）。

2026-08-02 按它改了两个（都在 `codex_config.rs`）：

```rust
pub const CC_SWITCH_CODEX_OFFICIAL_PROXY_PROVIDER_ID: &str = "loongport-official";
const LEGACY_OFFICIAL_PROXY_PROVIDER_IDS: &[&str] = &["cc-switch-official"];
pub fn is_official_proxy_provider_id(id: &str) -> bool { /* 认新旧两个 */ }
```

不留兜底的后果不是报错，是**静默失效**：老用户 `config.toml` 里写的是旧标记，
认不出来 ⇒ 崩溃后的兜底清理跳过它 ⇒ codex 一直指着一个不再监听的本地端口，
而用户完全不知为何连不上。

那两个 legacy 数组**只增不删** —— 每一项都对应「某个版本的用户机器上可能存在的值」。

## 三点六、⚠️ 同一事实散在多处 = 静默失效的温床

**本轮最贵的教训**：deeplink scheme 声明在**三处**，其中 `Info.plist` 漏改还写着
`ccswitch`，而另两处已是 `loongport` ⇒ 系统把 `ccswitch://` 交给我们、代码不认；
`loongport://` 系统压根不路由给我们 ⇒ **deeplink 导入完全失效，且不报任何错**。

这类问题的共性：**编译器管不到非 Rust 文件**（`.plist` / `.json` / `.ts`），
不一致时不崩不报，只是功能悄悄没了。

**通用解法：加一条 `include_str!` 比对的测试。** 已有两道，照它们的形状加新的：

| 闸 | 守什么 |
|---|---|
| `deeplink::scheme_consistency_tests` | `APP_SCHEME` 必须同时在 `Info.plist` 与 `tauri.conf.json` 里 |
| `operator::managed::prefix_matches_the_frontend_copy` | `MANAGED_ID_PREFIX` 必须与 `src/config/constants.ts` 一致 |

**新增任何「跨语言/跨文件的同一事实」时，一并加闸** —— 否则它迟早分叉，
而分叉那天没人会收到通知。已知还有一处同类：`OFFICIAL_WEBSITE`
（`tray.rs` 与 `constants.ts` 各一份，改一边记得改另一边）。

## 四、验收：六道闸

任何改动收尾都要过（CI 跑的就是这些，本地先过一遍省得来回）：

```
cd src-tauri
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd ..
npx tsc --noEmit && npx prettier --check "{src,tests}/**/*.{js,jsx,ts,tsx,css,json,html}" && npx vitest run
```

两个坑（都踩过）：

- **`cargo` 不在默认 PATH 里**，先 `export PATH="$HOME/.cargo/bin:$PATH"`，
  否则拿到的是 `command not found` 而非真实结果。
- **prettier 的 glob 必须与 CI 一致**（`"{src,tests}/**/*.{js,jsx,ts,tsx,css,json,html}"`，
  即 `package.json` 的 `format:check`）。别缩成 `src/**` —— 那会漏掉
  `tests/` 与 `.html`，本地全绿而 CI 红（2026-08-07 实测：`tests/components/…`
  的格式问题本地闸从来没扫到，合并时才被线上拦下）。直接跑
  `pnpm format:check` 最省事，别手写 glob。

**`cargo test` / `clippy` 全绿不代表能打包** —— CI 的 Backend Checks 不跑 `tauri build`，
Tauri 的 npm↔crate 版本校验只在打包时触发（已踩过，见 `ca82a908`）。

### 合并到远程 main：走 PR，且要过线上 4 个必需检查

改动要进 `main` 一律走 PR（`gh pr create`），不要直接 push 到 `main`。
**线上闸门是 4 个必需检查**（`Frontend Checks` + 三平台 `Backend Checks`，
内容就是上面那六道闸，见 `.github/workflows/ci.yml`）—— 本地全绿不代表远程会绿，
外部改动（fork PR）的验证点只有它。合并方式与仓库惯例一致用 merge commit
（dependabot 的自动合才是 squash，见下）。

标准流程（本地六道闸全过之后）：

```
git fetch origin main && git checkout -b fix/xxx origin/main
# ...改动 + 本地六道闸...
git push -u origin fix/xxx
gh pr create --base main --head fix/xxx --title "..." --body-file pr_body.md
gh pr merge fix/xxx --auto --merge    # 等 4 个必需检查全绿后自动合
```

`--auto` 只负责"检查绿了自动合"，**一道闸都没省**：main 的分支保护把 4 个
必需检查设为 required，任何一个不过都不会合。`--merge`（merge commit）是
人工 PR 的惯例；`dependabot-auto-merge.yml` 里那条用 `--squash` 是给
dependabot 的，别照搬。PR 模板在 `.github/pull_request_template.md`。
**main 只接受通过 PR 的改动** —— 这条与 design 仓无关（流程知识跟着代码走，
不抄进档案仓，见全局准则 §1.4 唯一数据源）。

### 打包与产物归档：唯一源在 LOONGPORT.md，这里只指路

打包命令、DMG 坑、产物路径、归档约定（mac 落 `~/下载`、windows 落 `D:\`）
**全部收在 [LOONGPORT.md](LOONGPORT.md) 的打包章节**，这里是**唯一一份**，
别在 CLAUDE.md 里复制第二遍（见全局准则 §1.4）。要打包时去读那份。

唯一属于 CLAUDE.md（维护视角）的是上面那条警告：**`cargo test` / `clippy` 全绿
不代表能打包** —— Tauri 的 npm↔crate 版本校验只在打包时触发。
