//! 主卡下方抽屉里的页面内容：上报数据（表单/成功页）、999/商人/BOSS 时分选择。
//!
//! 这些页面**不是独立窗口**，而是画在主卡同一窗口内的下方抽屉区（见 app.rs）。
//! 因此它们不再需要 deferred 子视口 / 隐藏主卡 / ViewportCommand，关闭 = 置回
//! `shared.page = None` 让抽屉收回即可。闭包仍传 `Arc<Mutex<Shared>>`，内容自己加锁。

pub mod pickers;
pub mod report;
