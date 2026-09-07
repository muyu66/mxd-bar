//! 与 UI/平台无关的纯函数：千分位格式化、计时显示。
//! 全部可脱离 egui 单元测试。

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

/// 千分位格式化，如 33622 -> "33,622"。
pub fn thousands(n: i64) -> String {
    let negative = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if negative {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 秒数 → 简写（向下取整）：`33s` / `55m` / `12h`。要求 secs≥0。
pub fn humanize_down(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// 秒数 → 时钟式显示（随大小升格）：`33s` → `1:05` → `1:00:00`。
/// <60s 带 `s` 后缀；≥60s 为 `M:SS`；≥1h 为 `H:MM:SS`（主卡"测试时间"）。
pub fn fmt_clock(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

// ---------------------------------------------------------------------------
// 三个计时按钮的显示状态（输入 now，便于测试）
// ---------------------------------------------------------------------------

/// 一个计时按钮当前要展示的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerUi {
    pub text: String,
    /// 需要红色（过期/待打卡）。
    pub red: bool,
    /// 需要绿色（商人已刷新）。
    pub green: bool,
    /// 抖动（仅 999 待打卡用）。
    pub jitter: bool,
}

/// 下一个“从 now 起还没到”的 HH:MM 时刻：今天若还没到用今天，已到则用明天同一时分。
/// 例：now=今天 10:00、t=09:00 → 明天 09:00；now=今天 08:00、t=09:00 → 今天 09:00。
pub fn next_occurrence(now: NaiveDateTime, t: NaiveTime) -> NaiveDateTime {
    let cand = now.date().and_time(t);
    if cand > now {
        cand
    } else {
        (now.date() + Duration::days(1)).and_time(t)
    }
}

/// 神秘商人未设置周期时的默认时长（05:59 = 每 5h59m 一轮）。
fn merchant_default() -> NaiveTime {
    NaiveTime::from_hms_opt(5, 59, 0).unwrap()
}

/// 商人每次"归零刷新"后，仍显示绿色“已刷新”的时长（秒）。
const MERCHANT_REFRESH_SHOW_S: i64 = 120;

/// 999打卡：一次性倒计时到“下一次 HH:MM 时刻”。设置后一直倒数；
/// 到点（含该时刻已经过去而尚未重新确认）→ 红色“999 待打卡”抖动，
/// 直到用户再次点开并确定（此时自动把目标滚到下一次）才恢复倒数。
pub fn punch_view(now: NaiveDateTime, t: Option<(NaiveDate, NaiveTime)>) -> TimerUi {
    let no = |text: &str| TimerUi { text: text.into(), red: false, green: false, jitter: false };
    match t {
        None => no("999打卡 无"),
        Some((d, tm)) => {
            let diff = (d.and_time(tm) - now).num_seconds();
            if diff > 0 {
                TimerUi { text: format!("999打卡 {}", humanize_down(diff)), red: false, green: false, jitter: false }
            } else {
                // 目标时刻已过 → 待打卡（红色抖动），直到重新确认滚到下一次。
                TimerUi { text: "999 待打卡".into(), red: true, green: false, jitter: true }
            }
        }
    }
}

/// 神秘商人：以“最近一次确定”(anchor) 为锚点的**相对循环倒计时**，与应用几点/哪天无关。
/// 周期 = 用户设定的时分（未设定默认 05:59）。从锚点起满一个周期就“刷新”一次：
/// 刚刷新（含之后约 2 分钟）显示绿色“已刷新”，随后自动回到周期值重新倒数；
/// 永远不会卡住——比如设 05:59 后在任意时刻设好，按钮立刻从 5h59m 起倒数。
/// 完全没有设定过（无锚点）→ 显示“神秘商人 无”。
pub fn merchant_view(
    now: NaiveDateTime,
    t: Option<NaiveTime>,
    anchor: Option<NaiveDateTime>,
) -> TimerUi {
    let no = |text: &str| TimerUi { text: text.into(), red: false, green: false, jitter: false };
    let Some(a) = anchor else {
        return no("神秘商人 无");
    };
    let period = t.unwrap_or_else(merchant_default).num_seconds_from_midnight() as i64;
    if period <= 0 {
        return no("神秘商人 无");
    }
    let s = (now - a).num_seconds().max(0);
    let since = s % period; // 距上一次“刷新”点经过的秒数
    let cycles = s / period; // 已完整走完几轮
    if cycles >= 1 && since < MERCHANT_REFRESH_SHOW_S {
        return TimerUi { text: "神秘商人 已刷新".into(), red: false, green: true, jitter: false };
    }
    let remain = (period - since).max(0);
    TimerUi { text: format!("神秘商人 {}", humanize_down(remain)), red: false, green: false, jitter: false }
}

/// BOSS：正向秒表，只记录“当前时分 − 设定时分”累计时长；
/// 满 99 分钟 → 作废“BOSS 无”。不倒数（设了未来时分按 0s 起算）。
pub fn boss_view(now: NaiveDateTime, t: Option<(NaiveDate, NaiveTime)>) -> TimerUi {
    let no = |text: &str| TimerUi { text: text.into(), red: false, green: false, jitter: false };
    match t {
        None => no("BOSS 无"),
        Some((d, tm)) => {
            let base = d.and_time(tm);
            let elapsed = (now - base).num_seconds().max(0);
            if elapsed >= 99 * 60 {
                no("BOSS 无") // 废弃
            } else {
                TimerUi { text: format!("BOSS {}", humanize_down(elapsed)), red: false, green: false, jitter: false }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(33622), "33,622");
        assert_eq!(thousands(-1234), "-1,234");
        assert_eq!(thousands(516103), "516,103");
    }

    #[test]
    fn humanize_floors() {
        assert_eq!(humanize_down(0), "0s");
        assert_eq!(humanize_down(33), "33s");
        assert_eq!(humanize_down(59), "59s");
        assert_eq!(humanize_down(60), "1m");
        assert_eq!(humanize_down(22 * 60), "22m");
        assert_eq!(humanize_down(2 * 3600), "2h");
        assert_eq!(humanize_down(12 * 3600 + 300), "12h");
    }

    #[test]
    fn clock_scales_units() {
        // <60s：纯秒带 s 后缀
        assert_eq!(fmt_clock(0), "0s");
        assert_eq!(fmt_clock(5), "5s");
        assert_eq!(fmt_clock(59), "59s");
        // ≥1min：分:秒
        assert_eq!(fmt_clock(60), "1:00");
        assert_eq!(fmt_clock(65), "1:05");
        assert_eq!(fmt_clock(3599), "59:59");
        // ≥1h：时:分:秒
        assert_eq!(fmt_clock(3600), "1:00:00");
        assert_eq!(fmt_clock(3665), "1:01:05");
        assert_eq!(fmt_clock(7200 + 123), "2:02:03");
    }

    // —— 计时显示 ——
    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }
    fn tm(h: u32, mi: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, mi, 0).unwrap()
    }

    #[test]
    fn next_occurrence_rolls_day() {
        // 今天 10:00，目标 09:00 已过 → 明天 09:00
        let now = dt(2026, 9, 3, 10, 0);
        assert_eq!(next_occurrence(now, tm(9, 0)), dt(2026, 9, 4, 9, 0));
        // 今天 08:00，目标 09:00 未到 → 今天 09:00
        let now2 = dt(2026, 9, 3, 8, 0);
        assert_eq!(next_occurrence(now2, tm(9, 0)), dt(2026, 9, 3, 9, 0));
        // 恰好到点（相等）算已过 → 明天
        let now3 = dt(2026, 9, 3, 9, 0);
        assert_eq!(next_occurrence(now3, tm(9, 0)), dt(2026, 9, 4, 9, 0));
    }

    #[test]
    fn punch_states() {
        let now = dt(2026, 9, 3, 20, 0);
        // 未设
        let v = punch_view(now, None);
        assert_eq!(v.text, "999打卡 无");
        assert!(!v.red);
        // 今天 20:03 未到 → 3m
        let v = punch_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 3).unwrap(), tm(20, 3))));
        assert_eq!(v.text, "999打卡 3m");
        assert!(!v.red);
        // 今天 20:00 已到（相等即过）→ 红色待打卡
        let v = punch_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 3).unwrap(), tm(20, 0))));
        assert_eq!(v.text, "999 待打卡");
        assert!(v.red && v.jitter);
        // 昨天设定的 → 已过 → 待打卡
        let v = punch_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(), tm(23, 0))));
        assert!(v.red);
    }

    const P: i64 = 5 * 3600 + 59 * 60; // 05:59 = 21540s，测试里复用的一个周期

    #[test]
    fn merchant_unset_is_none() {
        // 没设定过（无锚点）→ 无
        let now = dt(2026, 9, 3, 14, 0);
        let v = merchant_view(now, None, None);
        assert_eq!(v.text, "神秘商人 无");
        assert!(!v.green);
    }

    #[test]
    fn merchant_cycles_from_anchor() {
        // 14:00 设 05:59 → 立刻从 5h59m 开始倒数（约 5h，格式向下取整）
        let a = dt(2026, 9, 3, 14, 0);
        let just = a + Duration::seconds(60);
        let v = merchant_view(just, Some(tm(5, 59)), Some(a));
        assert_eq!(v.text, "神秘商人 5h");
        assert!(!v.green);
        // 过了整整一个周期 +30s → 刚刷新 → 绿色
        let fresh = a + Duration::seconds(P + 30);
        let v = merchant_view(fresh, Some(tm(5, 59)), Some(a));
        assert_eq!(v.text, "神秘商人 已刷新");
        assert!(v.green);
        // 刷新窗口(120s)过了 → 从 05:59 重新倒数
        let later = a + Duration::seconds(P + 200);
        let v = merchant_view(later, Some(tm(5, 59)), Some(a));
        assert_eq!(v.text, "神秘商人 5h");
        assert!(!v.green);
        // 第二轮照样循环：2P+30s 又绿一次
        let fresh2 = a + Duration::seconds(2 * P + 30);
        let v = merchant_view(fresh2, Some(tm(5, 59)), Some(a));
        assert!(v.green);
    }

    #[test]
    fn merchant_default_period_when_time_unset() {
        // 有锚点但没存周期 → 按默认 05:59 循环
        let a = dt(2026, 9, 3, 14, 0);
        let now = a + Duration::seconds(60);
        let v = merchant_view(now, None, Some(a));
        assert_eq!(v.text, "神秘商人 5h");
        assert!(!v.green);
    }

    #[test]
    fn merchant_custom_cycle() {
        // 设 01:00(3600s)：半分钟前开始 → 还剩 ~59m
        let a = dt(2026, 9, 3, 12, 0);
        let now = a + Duration::seconds(30);
        let v = merchant_view(now, Some(tm(1, 0)), Some(a));
        assert_eq!(v.text, "神秘商人 59m");
        assert!(!v.green);
        // 过了整整一小时 → 绿色刷新
        let fresh = a + Duration::seconds(3600 + 5);
        let v = merchant_view(fresh, Some(tm(1, 0)), Some(a));
        assert!(v.green);
    }

    #[test]
    fn boss_forward_only() {
        let now = dt(2026, 9, 3, 14, 0);
        assert_eq!(boss_view(now, None).text, "BOSS 无");
        // 22 分钟前开始 → 正向累计 22m
        let v = boss_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 3).unwrap(), tm(13, 38))));
        assert_eq!(v.text, "BOSS 22m");
        // 超过 99 分钟 → 废弃
        let v = boss_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 3).unwrap(), tm(12, 0))));
        assert_eq!(v.text, "BOSS 无");
        // 昨天 → 早已超过 99 分钟 → 废弃
        let v = boss_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(), tm(14, 0))));
        assert_eq!(v.text, "BOSS 无");
        // 未来时分（不应倒数）→ 按 0s 起算
        let v = boss_view(now, Some((NaiveDate::from_ymd_opt(2026, 9, 3).unwrap(), tm(15, 0))));
        assert_eq!(v.text, "BOSS 0s");
    }
}
