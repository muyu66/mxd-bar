//! 与 UI/平台无关的纯函数：千分位格式化、计时显示。
//! 全部可脱离 egui 单元测试。

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};

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
// 两个计时按钮（999打卡/BOSS）的显示状态（输入 now，便于测试）
// ---------------------------------------------------------------------------

/// 一个计时按钮当前要展示的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerUi {
    pub text: String,
    /// 需要红色（过期/待打卡）。
    pub red: bool,
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

/// 999打卡：一次性倒计时到“下一次 HH:MM 时刻”。设置后一直倒数；
/// 到点（含该时刻已经过去而尚未重新确认）→ 红色“999 待打卡”抖动，
/// 直到用户再次点开并确定（此时自动把目标滚到下一次）才恢复倒数。
pub fn punch_view(now: NaiveDateTime, t: Option<(NaiveDate, NaiveTime)>) -> TimerUi {
    let no = |text: &str| TimerUi { text: text.into(), red: false, jitter: false };
    match t {
        None => no("999打卡 无"),
        Some((d, tm)) => {
            let diff = (d.and_time(tm) - now).num_seconds();
            if diff > 0 {
                TimerUi { text: format!("999打卡 {}", humanize_down(diff)), red: false, jitter: false }
            } else {
                // 目标时刻已过 → 待打卡（红色抖动），直到重新确认滚到下一次。
                TimerUi { text: "999 待打卡".into(), red: true, jitter: true }
            }
        }
    }
}

/// BOSS：正向秒表，只记录“当前时分 − 设定时分”累计时长；
/// 满 99 分钟 → 作废“BOSS 无”。不倒数（设了未来时分按 0s 起算）。
pub fn boss_view(now: NaiveDateTime, t: Option<(NaiveDate, NaiveTime)>) -> TimerUi {
    let no = |text: &str| TimerUi { text: text.into(), red: false, jitter: false };
    match t {
        None => no("BOSS 无"),
        Some((d, tm)) => {
            let base = d.and_time(tm);
            let elapsed = (now - base).num_seconds().max(0);
            if elapsed >= 99 * 60 {
                no("BOSS 无") // 废弃
            } else {
                TimerUi { text: format!("BOSS {}", humanize_down(elapsed)), red: false, jitter: false }
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
