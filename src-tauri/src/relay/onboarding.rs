//! 新人引导（onboarding）：新用户首次打开 LoongPort 时，自动打开官方站
//! （BestAPI）的注册页，让「注册即用」这条路径一步到位。
//!
//! ## 这个模块的边界（为什么单独收拢）
//!
//! 新人引导的**策略**（什么时候弹、弹哪个站、页面上说什么）预期还会调整；
//! 而它依赖的**机制**（登录窗、协议探测、优惠码预填、凭据回传）是 `relay`
//! 里既有的稳定链路。所以策略全部收在本模块，机制一行不改 —— 调整引导
//! 行为时只动这里，不碰 `commands::relay` 的主流程。
//!
//! 具体分工：
//! - 本模块：官方站 origin、注册窗完成事件名、新人礼包横幅脚本；
//! - [`crate::commands::onboarding`]：判据（一次性标志 + 还没有任何账号）、
//!   打开窗口的调度；
//! - [`crate::commands::relay`]：`BrowserEntrySource::Onboarding` 入口把本模块的
//!   横幅脚本接进既有的 `browser_import` 窗口。
//!
//! ## 优惠码不在这里
//!
//! 注册页的优惠码预填（`LOONGPORT`）由既有的 `promo` 链路负责：
//! `browser_import` 对任何站点都会解析优惠码并注入
//! [`login`] 的预填脚本，本模块不复制那份码表。

/// 新人引导自动打开的官方站。BestAPI 是维护者自己的站，是「注册即用 +
/// 新人大礼包」承诺的承载方；与优惠码表（`promo::PROMO_CODES`）里键的
/// host 保持一致。
pub const OFFICIAL_SITE_ORIGIN: &str = "https://bestapi.store";

// 注册窗完成事件名见 `crate::events::ONBOARDING_REGISTER_COMPLETED`（跨语言
// 事件统一收在 `events`，有一致性闸守着）；发射点在 `commands::onboarding`。

/// 新人引导注册窗的标题。区别于手动的「添加中转站 …」：新用户没有上下文，
/// 「添加中转站 bestapi.store」对他是一句黑话；「注册 + 礼包」才说得清这个
/// 窗口为什么自己弹出来。
pub fn register_window_title() -> String {
    "注册 BestAPI — 新人大礼包".to_string()
}

/// 注册页顶部的「新人礼包」横幅脚本，作为 `initialization_script` 注入
/// 新人引导的注册窗。
///
/// ## 文案是写死的中文，不随 app 语言切换
///
/// 与 [`login::register_hint_banner_snippet`]「复用页面自己的文案」不同，这条
/// 是维护者要求的官方推广语（「注册即用，官方赠送新人大礼包」）—— 它本来就是
/// 针对 BestAPI（中文站）说的，不存在四语版本；BestAPI 界面切到英文时这条
/// 横幅仍是中文，与「站点自己的中文推广位」性质相同，不算语言错乱。
///
/// ## 与「已有账号」横幅的堆叠
///
/// 注册页上还有一条 [`login`] 注入的「已有账号？去登录」横幅
/// （`#loongport-register-hint`，协议识别后才会出现）。两条都是 fixed 定位，
/// 本横幅**让位**：每次同步时若检测到对方存在，把自己的 `top` 挪到对方高度
/// 之下，并把 `body.paddingTop` 补到两条横幅的总高度。对方先撤时，下一轮
/// 轮询（500ms）自动回落到顶部。
///
/// 其余行为沿用 `register_hint_banner_snippet` 已经验证过的模式，理由相同：
/// 只在 `/register` 路径显示（含 hash 路由）、SPA 路由切换靠 500ms 轮询
/// 而不是 MutationObserver、必须有轮询上限（拿到凭据后窗口**有意不关**，
/// 用户会在里面继续看 dashboard）。
pub fn register_gift_banner_js() -> String {
    r##"
  // 新人礼包横幅。定位与让位规则见 Rust 侧 register_gift_banner_js 的文档。
  try {
    var GIFT_BANNER_ID = 'loongport-onboarding-gift';
    var HINT_BANNER_ID = 'loongport-register-hint';
    var GIFT_TEXT = '注册即用，官方赠送新人大礼包';

    var paddedByUs = false;

    function hintBannerHeight() {
      var hint = document.getElementById(HINT_BANNER_ID);
      return hint && hint.offsetParent !== null ? hint.offsetHeight : 0;
    }

    function removeBanner() {
      var old = document.getElementById(GIFT_BANNER_ID);
      if (old) old.remove();
      // 离开注册页要还原（拿到凭据后窗口有意不关，用户还会逛 dashboard）。
      // 只还原自己设的那次：paddingTop 为空串才可能是我们设的。
      if (paddedByUs) {
        document.body.style.paddingTop = '';
        paddedByUs = false;
      }
    }

    function syncBanner() {
      var onRegister = window.location.pathname.indexOf('/register') !== -1
        || window.location.hash.indexOf('/register') !== -1;
      if (!onRegister) { removeBanner(); return; }

      var bar = document.getElementById(GIFT_BANNER_ID);
      if (!bar) {
        bar = document.createElement('div');
        bar.id = GIFT_BANNER_ID;
        bar.setAttribute('role', 'status');
        // 内联样式：站点的 CSS 类名不稳定，横幅必须一直看得见（同既有横幅）。
        bar.style.cssText = [
          'position:fixed', 'left:0', 'right:0', 'z-index:2147483647',
          'display:flex', 'align-items:center', 'justify-content:center',
          'padding:10px 16px', 'background:#16a34a', 'color:#fff',
          'font:500 14px/1.5 system-ui,-apple-system,sans-serif',
          'box-shadow:0 1px 3px rgba(0,0,0,.2)'
        ].join(';');
        var text = document.createElement('span');
        text.textContent = GIFT_TEXT;
        bar.appendChild(text);
        document.body.appendChild(bar);
      }

      // 让位于「已有账号」横幅：它在就挪到它下面，不在就回顶部。
      bar.style.top = hintBannerHeight() + 'px';

      // 两横幅的总高度都算进 paddingTop，页面顶部内容才不被盖住。
      // 只在站点自己没设过这个属性时才动它，并记下「是我们设的」。
      if (document.body.style.paddingTop === '') {
        paddedByUs = true;
      }
      if (paddedByUs) {
        document.body.style.paddingTop = (hintBannerHeight() + bar.offsetHeight) + 'px';
      }
    }

    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', syncBanner);
    } else {
      syncBanner();
    }
    var polls = 0;
    var timer = setInterval(function () {
      polls++;
      if (polls > 600) {
        clearInterval(timer);
        removeBanner();
        return;
      }
      syncBanner();
    }, 500);
  } catch (e) {
    // 横幅是提示，不是注册必需的一步。它坏了绝不能影响凭据回传。
    console.warn('[LoongPort] 新人礼包横幅未能显示:', e);
  }
"##
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gift_banner_uses_dedicated_element_id_and_requested_copy() {
        let js = register_gift_banner_js();
        // 文案逐字来自维护者的要求；改文案必须是有意识的决定，不是顺手。
        assert!(js.contains("注册即用，官方赠送新人大礼包"));
        assert!(js.contains("loongport-onboarding-gift"));
        // 与「已有账号」横幅的堆叠关系挂在对方的 id 上，改对方 id 时这里要跟着看。
        assert!(js.contains("loongport-register-hint"));
    }
}
