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

## 四、验收：六道闸

任何改动收尾都要过（CI 跑的就是这些，本地先过一遍省得来回）：

```
cd src-tauri
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd ..
npx tsc --noEmit && npx prettier --check "src/**/*.{js,jsx,ts,tsx,css,json}" && npx vitest run
```

两个坑（都踩过）：

- **`cargo` 不在默认 PATH 里**，先 `export PATH="$HOME/.cargo/bin:$PATH"`，
  否则拿到的是 `command not found` 而非真实结果。
- **prettier 要用 CI 那个 glob**（`"src/**/*.{js,jsx,ts,tsx,css,json}"`）。
  直接 `prettier --check src` 会把 `src/index.html` 也扫进来报 warn，那个不在闸内。

**`cargo test` / `clippy` 全绿不代表能打包** —— CI 的 Backend Checks 不跑 `tauri build`，
Tauri 的 npm↔crate 版本校验只在打包时触发（已踩过，见 `ca82a908`）。
