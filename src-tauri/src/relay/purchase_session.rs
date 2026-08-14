//! 充值窗口对 NewAPI refresh credential 轮换的**独占协调器**。
//!
//! ## 为什么需要它
//!
//! NewAPI 的 refresh cookie 是**一次性轮换**的：每次 `/api/user/auth/refresh` 成功后，
//! 服务端作废旧 cookie、发一颗新的。充值窗口（`purchase`）启动时会把当时的 refresh
//! cookie 种进 WebView；如果后台流程（provision / 余额 / 刷新经过 `usable_relay`）在窗口
//! 存活期间并发跑了一次续期，轮换出的新 cookie 只落进本地 DB，**窗口里那颗立刻作废**
//! —— 用户充值到一半被踢回登录页。这个模块把「谁有权触发 refresh 轮换」收成一个
//! app 级的独占租约（lease）。
//!
//! ## 三条设计裁决
//!
//! 1. **按 `relay_id`（i64）键控，绝不按 host** —— 一行 relay 是「站点 × 账号」，
//!    同一站开两个账号是合法用法；按 host 键控会让 A 账号的充值窗口把 B 账号的
//!    续期也拦下（或反过来），两个本该独立的会话互相踩。
//! 2. **lease 是 RAII（Drop 释放），不提供手动 release** —— 释放点只有「lease 值被
//!    drop」这一个。充值窗口的生命周期横跨 Tauri 窗口事件（创建失败 / 用户关窗 /
//!    崩溃 unwind），任何一条路径忘了调手动 release，那个账号就**永远**开不了第二次
//!    充值窗口 —— 死锁。挂在值上让编译器保证：值没了，锁就还了。
//! 3. **`Mutex<HashSet<i64>>` 而不是 per-relay 锁** —— 集合里最多同时存在几个 id
//!    （用户开着的充值窗口数），临界区是 contains/insert/remove，纳秒级；为每个
//!    relay 维护一把锁（外加创建/回收它们自己的同步问题）是给纳秒级临界区配调度
//!    场，过度设计。
//!
//! 这个模块**不持有也不感知凭据值**：它只认 relay id，错误文案不含任何 secret。

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::AppError;

/// 谁正在持有哪个 relay 的 refresh 轮换独占权。
///
/// 挂在 `AppState` 上（app 级单例）：充值窗口与后台续期是**两条互不相识的调用链**，
/// 只能在一个双方都够得着的注册表里会合。
#[derive(Default)]
pub struct PurchaseSessionCoordinator {
    active: Mutex<HashSet<i64>>,
}

impl PurchaseSessionCoordinator {
    /// 为 `relay_id` 取一份独占 lease；该 relay 已有活跃 lease 时报错（不排队、不等待）。
    ///
    /// 排队（await 直到别人释放）看似更友好，但排队意味着「开第二个充值窗口」静默
    /// 变成「等第一个关掉再开」—— 用户看不出任何提示，只觉得窗口开不出来。
    /// 立刻报错让上层把「窗口已打开」说清楚。
    pub fn try_acquire(self: &Arc<Self>, relay_id: i64) -> Result<PurchaseSessionLease, AppError> {
        let mut active = self.lock_active();
        if active.contains(&relay_id) {
            return Err(AppError::Config("这个账号的充值窗口已经打开".into()));
        }
        active.insert(relay_id);
        Ok(PurchaseSessionLease {
            coordinator: Arc::clone(self),
            relay_id,
        })
    }

    /// `relay_id` 是否有活跃的充值 lease。
    ///
    /// 消费方是 `usable_relay` 的续期闸（充值窗口开着 ⇒ 后台不得轮换 refresh cookie）。
    pub fn is_active(&self, relay_id: i64) -> bool {
        self.lock_active().contains(&relay_id)
    }

    /// 只移除 `relay_id` 自己 —— lease 的 Drop 独占调用，别处不可达。
    fn release(&self, relay_id: i64) {
        self.lock_active().remove(&relay_id);
    }

    /// 拿锁，锁中毒时恢复内层数据继续用。
    ///
    /// 中毒只意味着「持有锁的线程 panic 过」，而这里的临界区是单步集合操作，
    /// 不存在跨语句不变量 —— 内层 `HashSet` 要么改了要么没改，不会半更新。
    /// 把锁毒当致命错误会让一个无关线程的 panic 永久瘫痪所有充值窗口。
    fn lock_active(&self) -> MutexGuard<'_, HashSet<i64>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 一次充值会话的独占权。drop 时自动归还（见模块文档的 RAII 裁决）。
pub struct PurchaseSessionLease {
    coordinator: Arc<PurchaseSessionCoordinator>,
    relay_id: i64,
}

impl Drop for PurchaseSessionLease {
    fn drop(&mut self) {
        self.coordinator.release(self.relay_id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{PurchaseSessionCoordinator, PurchaseSessionLease};

    #[test]
    fn lease_is_exclusive_per_relay_and_released_on_drop() {
        let coordinator = Arc::new(PurchaseSessionCoordinator::default());
        let first = coordinator.try_acquire(7).unwrap();
        assert!(coordinator.is_active(7));
        assert!(coordinator.try_acquire(7).is_err());
        assert!(coordinator.try_acquire(8).is_ok());
        assert!(!coordinator.is_active(9));
        drop(first);
        assert!(!coordinator.is_active(7));
        assert!(coordinator.try_acquire(7).is_ok());
    }

    /// 充值窗口 panic（窗口销毁路径最常见的异常出口）也必须把 lease 还回去，
    /// 否则那个账号从此再也开不了充值窗口 —— RAII 的核心承诺。
    #[test]
    fn lease_released_even_when_the_holder_panics() {
        let coordinator = Arc::new(PurchaseSessionCoordinator::default());

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lease: PurchaseSessionLease = coordinator.try_acquire(7).unwrap();
            panic!("充值窗口在持有 lease 时崩溃");
        }));
        assert!(outcome.is_err(), "测试前提：panic 确实发生了");

        assert!(
            !coordinator.is_active(7),
            "panic 展开（unwind）时 Drop 必须已把 lease 释放"
        );
        assert!(
            coordinator.try_acquire(7).is_ok(),
            "lease 释放后同一 relay 必须能重新 acquire"
        );
    }
}
