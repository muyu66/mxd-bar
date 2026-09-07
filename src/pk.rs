//! 实时效率 PK 上报后台线程：每 10s 向服务器上报"当前预估 EXP/h"，换回名次写进
//! `Shared.pk`，主卡最左区域据此画 `第x名` 标签（见 app.rs paint_pk）。
//!
//! 规则（用户 2026-09-07 定稿，服务端约定见根目录 pk-api.md）：
//! - **何时上报**：只在"确有可上报的实时经验速率"时报——即采样线程最近 30s 内成功
//!   读过一次 EXP 且已算出 预估EXP/时（`exp.n_hour>0`）。切走/最小化/关游戏导致的
//!   "暂停"会使读数变陈旧 → 不再上报（服务端据此把本设备自然挤出 PK 榜），
//!   名次也随后被清掉显示 `-`。
//! - **上报内容**：`{ board_id, ts, exp_per_hour }`，与 v2 上报同一套 `Bearer` JWT 鉴权
//!   （token 的 sub 即主板 MD5 uid，服务端以它为设备身份，不信 body）。
//! - **401**：缓存 token 失效 → 强制换新重试一次（与 ui/report.rs 上报同策略）。
//! - **名次清理**：连续多轮（每轮 10s）没能上报成功（离线或网络失败）就把名次清成 `-`，
//!   避免把陈旧的 `第x名` 一直摆着误导。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::net;
use crate::state::Shared;

/// 上报周期：每 10 秒一次（服务端要求 >5s，避免撞限频 429）。
pub(crate) const REPORT_EVERY: Duration = Duration::from_secs(10);
/// 采样读数"新鲜"窗口：此刻之前仍在被当作正在刷怪的读数（近似 6 个 5s tick）。
const FRESH: Duration = Duration::from_secs(30);
/// 连续多少轮（每轮 10s）没有一次成功上报后，清掉名次显示 `-`。≈50s。
const CLEAR_ROUNDS: u32 = 5;

/// 是否有"可上报"的实时速率：有样本（n_hour>0）且最近一次采样在新鲜窗口内。
/// 拆成纯函数便于单测。
fn report_live(n_hour: u32, updated: Option<Instant>, now: Instant) -> bool {
    n_hour > 0 && updated.is_some_and(|t| now.saturating_duration_since(t) < FRESH)
}

/// 起一个后台线程维护 PK。进程退出随主线程一起结束（与 token_keeper 同一模式）。
pub fn spawn(shared: Arc<Mutex<Shared>>) {
    std::thread::spawn(move || {
        // 连续无成功上报的轮数（离线或网络失败都算）。够 CLEAR_ROUNDS 就清名次。
        let mut stale: u32 = 0;
        loop {
            std::thread::sleep(REPORT_EVERY);

            // —— 取"是否有可上报的实时速率 + 预估值"（一次短锁快照）——
            let (live, per_hour) = {
                let g = shared.lock().unwrap();
                let live = report_live(g.exp.n_hour, g.exp.updated, Instant::now());
                (live, g.exp.per_hour)
            };
            if !live {
                stale += 1;
                if stale >= CLEAR_ROUNDS {
                    clear_rank(&shared);
                }
                continue;
            }

            // —— 上报：成功 → 写名次；失败 → 记原因（keep 旧名次），连续失败也清 ——
            match report_once(&shared, per_hour) {
                Ok(rk) => {
                    stale = 0;
                    let mut g = shared.lock().unwrap();
                    g.pk.rank = Some(rk.rank);
                    g.pk.total = (rk.total > 0).then_some(rk.total);
                    g.pk.updated = Some(Instant::now());
                    g.pk.error = None;
                }
                Err(ae) => {
                    stale += 1;
                    #[cfg(debug_assertions)]
                    eprintln!("[mxd-bar] PK 上报失败: {}", ae.message);
                    shared.lock().unwrap().pk.error = Some(ae.message);
                    if stale >= CLEAR_ROUNDS {
                        clear_rank(&shared);
                    }
                }
            }
        }
    });
}

/// 换/取 token → POST。遇 401（token 失效）强制换新重试一次。
fn report_once(shared: &Arc<Mutex<Shared>>, per_hour: i64) -> Result<net::PkRank, net::ApiError> {
    let to_api = |e: String| net::ApiError { status: None, message: e };
    let base = {
        let g = shared.lock().unwrap();
        net::api_base(&g.cfg)
    };
    let token = net::ensure_device_token(shared).map_err(to_api)?;
    let payload = {
        let board_id = shared.lock().unwrap().uid.clone().unwrap_or_default();
        serde_json::json!({
            "board_id": board_id,
            "ts": chrono::Utc::now().timestamp(),
            "exp_per_hour": per_hour,
        })
    };
    match net::post_pk(base, &token, &payload) {
        Ok(r) => Ok(r),
        Err(ae) if ae.status == Some(401) => {
            // 缓存里的 token 被拒：清掉换新的再试一次
            let token2 = net::refresh_token(shared).map_err(to_api)?;
            net::post_pk(base, &token2, &payload)
        }
        Err(ae) => Err(ae),
    }
}

/// 清掉名次（离线太久 / 长时间上报失败 → 主卡回到 `-`）。
fn clear_rank(shared: &Arc<Mutex<Shared>>) {
    let mut g = shared.lock().unwrap();
    g.pk.rank = None;
    g.pk.total = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_needs_fresh_sample() {
        let now = Instant::now();
        // 无样本 → 不上报
        assert!(!report_live(0, Some(now - Duration::from_secs(2)), now));
        // 有样本但太久没采到（>30s）→ 不上报
        assert!(!report_live(3, Some(now - Duration::from_secs(31)), now));
        // 有样本且新鲜 → 上报
        assert!(report_live(3, Some(now - Duration::from_secs(2)), now));
        // 完全没有采样记录 → 不上报
        assert!(!report_live(3, None, now));
        // 恰好在窗口内（<30s）→ 上报；刚好 30s 边缘 → 过期不上报
        assert!(report_live(3, Some(now - Duration::from_secs(29)), now));
        assert!(!report_live(3, Some(now - Duration::from_secs(30)), now));
    }
}
