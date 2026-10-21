# LoongPort 服务与账号交互原型

确认版本：74ab6052f169ac4d39ced228e10ec0a483f5bcb6。纯静态演示，不连接真实账号、服务或凭据，不代表生产功能验收。
打开 dist/index.html 即可体验；也可在本目录运行 python3 -m http.server 8000 后访问 /dist/。
运行 node test-interactions.cjs 可复用 16 项 Node VM DOM-stub 交互逻辑检查，不是浏览器视觉测试。

## 交互
统一顶部品牌/版本标签，TRAE 中国版和国际版独立；MiniMax Code 国内与国际独立。账号、权益、授权各自隔离；地区 SG/US 不提升为版本。关闭仅隐藏导航，不删除账号或改变共同池；末尾 + 恢复或添加标签，不重复创建。
所有状态在内存，刷新重置。输入仅供演示，请勿填真实账号、密码或密钥。

## Logo 与名称来源
ChatGPT、MiniMax、OpenRouter、OpenCode、ZCode 使用 LoongPort 已有品牌 SVG，来源 ref 2296f9afc17f2eee9f20154418df005b02f3aef5 的 src/icons/extracted/；ZCode SVG 来自同目录 index.ts。
WorkBuddy：https://www.workbuddy.ai/ 的官方 Logo https://download.codebuddy.ai/web/workbuddy/2365138d79c29bb1cbce4644f5cbbffd2b561a5c/assets/logo.svg
Qoder：https://qoder.com/favIcon.svg
TRAE：https://www.trae.ai/ 官方 favicon https://lf16-web-neutral.traecdn.ai/obj/trae-ai-static/trae_website/favicon.png
TRAE 版本称谓：https://docs.trae.cn/ide_contact-us
MiniMax Code 名称：https://agent.minimax.cn/download 与 https://agent.minimax.io/download
MiniMax 账号范围：https://agent.minimax.io/docs/cli/quick-start
品牌资源仅用于产品识别；商标权归原权利人。保留各品牌名称与图标可访问文字。
