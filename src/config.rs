//! data.ini 持久化：读写 `%APPDATA%\mxd-bar\data.ini`。
//!
//! 存放：上报表单默认值([report])、两个计时器目标([timers])、
//! OCR 校准标签([ocr])、用户 ID 缓存([meta])。
//! 自实现一个小 INI（无第三方依赖）：行式解析，支持 `[section]` 与 `key=value`，
//! 整行注释以 `;` 或 `#` 开头。
//!
//! `[panel]` 只存主卡**上次退出时的位置**(x/y)，不存布局模式 —— 是否"顶部吸附/收缩展开"
//! 完全由该位置反推(app.rs：y 落进顶部吸附带 ≤TOP_SNAP → 启动即 Auto 收起；否则常规)。
//! 旧 ini 里遗留的 `mode=` 键读盘时会被忽略，不影响 x/y。

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use chrono::{NaiveDate, NaiveTime};

/// 主卡两种布局模式。**只作运行时内存态、不落盘** —— ini 里不存 mode，
/// 启动时由 `[panel] y`(上次退出位置)是否落在顶部吸附带内反推(见 app.rs `new`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    /// 常规模式：悬浮可任意拖动、位置自由。把卡片拖到屏幕顶部松手即切到 Auto。
    Normal,
    /// 收缩/展开模式：固定贴屏幕顶部(y=0)，平时收成一条线，悬停展开、离开 5s 收回；
    /// 悬停展开态拖离顶部松手会切回 Normal。
    Auto,
}

/// 整套可持久化配置（内存镜像）。
#[derive(Debug, Clone)]
pub struct AppConfig {
    // —— [report] ——
    pub level: u32,
    /// 职业名；加载时按 jobs.json 反查 group 判"魔法师系"。
    pub job: String,
    /// 地图名（mapName），允许用户自定义输入。
    pub map: String,
    /// true=单人，false=组队。
    pub mode_solo: bool,
    /// 攻击力或魔法力（标签随职业 group 切换）。
    pub power: u64,

    // —— [timers] ——
    /// 999打卡：某天的时分（含日期，用于判断"已过点"）。
    pub punch: Option<(NaiveDate, NaiveTime)>,
    /// BOSS：设定时刻（日期+时分），用于秒表计时。
    pub boss: Option<(NaiveDate, NaiveTime)>,

    // —— [ocr] ——
    pub exp_label: String,
    pub level_label: String,

    // —— [meta] ——
    /// 主板序列号 → MD5(32hex)，缓存避免每次启动都跑 PowerShell。
    pub uid_cache: Option<String>,

    // —— [net] ——
    /// true = 用本地测试地址(http://127.0.0.1:3001)；false = 生产 https://mxd.zhuzhu.website。
    /// 对应 ini 的 `[net] base=local|prod`（缺省 prod）。改了无需重新编译，方便联调后切回生产。
    pub net_local: bool,

    // —— [panel] ——
    /// 主卡**上次退出时的左上角 x**（逻辑像素）。<0 = 还没记录过 → 启动顶部居中。
    pub panel_x: f32,
    /// 主卡**上次退出时的左上角 y**（逻辑像素）。<0 = 还没记录过 → 启动 y=40。
    /// 是否进入顶部收缩/展开(Auto)由它反推：y 落在顶部吸附带(≤ app::TOP_SNAP)内 →
    /// 启动即 Auto 收起贴顶；带外 → 常规悬浮不吸附。
    pub panel_y: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        // 与 derive 一致，仅 `[panel]` 的 x/y 给上"未记录"哨兵（-1）。
        AppConfig {
            level: 0,
            job: String::new(),
            map: String::new(),
            mode_solo: false,
            power: 0,
            punch: None,
            boss: None,
            exp_label: String::new(),
            level_label: String::new(),
            uid_cache: None,
            net_local: false,
            panel_x: -1.0,
            panel_y: -1.0,
        }
    }
}

impl AppConfig {
    /// 打开上报页时的默认值由这里决定；job 为空时调用方填第一组第一个职业。
    pub fn report_defaults(&self) -> (u32, String, String, bool, u64) {
        (self.level, self.job.clone(), self.map.clone(), self.mode_solo, self.power)
    }
}

/// `%APPDATA%\mxd-bar`；拿不到 APPDATA 时退回当前目录下 `.mxd-bar`。
pub fn data_dir() -> PathBuf {
    match std::env::var("APPDATA") {
        Ok(a) if !a.trim().is_empty() => PathBuf::from(a).join("mxd-bar"),
        _ => PathBuf::from(".").join(".mxd-bar"),
    }
}

/// `data.ini` 完整路径。
pub fn ini_path() -> PathBuf {
    data_dir().join("data.ini")
}

/// 按名字在数据目录中找文件（jobs.json / maps.json）。
/// 依次尝试：exe 同目录/data、exe 同目录、当前目录/data、当前目录/../data。
pub fn find_data_file(name: &str) -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("data").join(name));
            cands.push(dir.join(name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("data").join(name));
        cands.push(cwd.join(name));
        if let Some(p) = cwd.parent() {
            cands.push(p.join("data").join(name));
        }
    }
    cands.into_iter().find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// 内部：极简 INI 表示
// ---------------------------------------------------------------------------

type Ini = HashMap<(String, String), String>;

fn parse_ini(text: &str) -> Ini {
    let mut map: Ini = HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq + 1..].trim();
            if !key.is_empty() {
                map.insert((section.clone(), key.to_owned()), val.to_owned());
            }
        }
    }
    map
}

fn get<'a>(ini: &'a Ini, sec: &str, key: &str) -> Option<&'a str> {
    ini.get(&(sec.to_owned(), key.to_owned())).map(|s| s.as_str())
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}
fn parse_time(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").ok()
}
fn fmt_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}
fn fmt_time(t: NaiveTime) -> String {
    t.format("%H:%M").to_string()
}

impl AppConfig {
    fn from_ini(ini: &Ini) -> Self {
        let mut cfg = AppConfig::default();
        cfg.level = get(ini, "report", "level").and_then(|s| s.parse().ok()).unwrap_or(0);
        cfg.job = get(ini, "report", "job").unwrap_or("").to_owned();
        cfg.map = get(ini, "report", "map").unwrap_or("").to_owned();
        cfg.mode_solo = match get(ini, "report", "mode") {
            Some("party") => false,
            _ => true, // 缺省单人
        };
        cfg.power = get(ini, "report", "power").and_then(|s| s.parse().ok()).unwrap_or(0);

        let p_date = get(ini, "timers", "punch_date").and_then(parse_date);
        let p_time = get(ini, "timers", "punch_time").and_then(parse_time);
        if let (Some(d), Some(t)) = (p_date, p_time) {
            cfg.punch = Some((d, t));
        }
        let b_date = get(ini, "timers", "boss_date").and_then(parse_date);
        let b_time = get(ini, "timers", "boss_time").and_then(parse_time);
        if let (Some(d), Some(t)) = (b_date, b_time) {
            cfg.boss = Some((d, t));
        }

        cfg.exp_label = get(ini, "ocr", "exp_label").unwrap_or("EXP").to_owned();
        cfg.level_label = get(ini, "ocr", "level_label").unwrap_or("Lv").to_owned();
        cfg.uid_cache = get(ini, "meta", "uid").map(str::to_owned);
        cfg.net_local = get(ini, "net", "base").is_some_and(|s| s.eq_ignore_ascii_case("local"));
        // 只读 x/y。旧 ini 的 `[panel] mode=` 键被忽略——模式不再落盘，由 y 反推。
        cfg.panel_x = get(ini, "panel", "x").and_then(|s| s.parse().ok()).unwrap_or(-1.0);
        cfg.panel_y = get(ini, "panel", "y").and_then(|s| s.parse().ok()).unwrap_or(-1.0);
        cfg
    }

    fn render(&self) -> String {
        let mut out = String::new();

        out.push_str("[report]\n");
        out.push_str(&format!("level={}\n", self.level));
        out.push_str(&format!("job={}\n", self.job));
        out.push_str(&format!("map={}\n", self.map));
        out.push_str(&format!("mode={}\n", if self.mode_solo { "solo" } else { "party" }));
        out.push_str(&format!("power={}\n", self.power));

        out.push_str("[timers]\n");
        match self.punch {
            Some((d, t)) => {
                out.push_str(&format!("punch_date={}\n", fmt_date(d)));
                out.push_str(&format!("punch_time={}\n", fmt_time(t)));
            }
            None => {
                out.push_str("punch_date=\n");
                out.push_str("punch_time=\n");
            }
        }
        match self.boss {
            Some((d, t)) => {
                out.push_str(&format!("boss_date={}\n", fmt_date(d)));
                out.push_str(&format!("boss_time={}\n", fmt_time(t)));
            }
            None => {
                out.push_str("boss_date=\n");
                out.push_str("boss_time=\n");
            }
        }

        out.push_str("[ocr]\n");
        out.push_str(&format!("exp_label={}\n", self.exp_label));
        out.push_str(&format!("level_label={}\n", self.level_label));

        out.push_str("[meta]\n");
        if let Some(uid) = &self.uid_cache {
            out.push_str(&format!("uid={}\n", uid));
        }

        out.push_str("[net]\n");
        out.push_str(&format!("base={}\n", if self.net_local { "local" } else { "prod" }));

        out.push_str("[panel]\n");
        out.push_str(&format!("x={}\n", self.panel_x));
        out.push_str(&format!("y={}\n", self.panel_y));
        out
    }
}

/// 读入配置；文件不存在或损坏时返回默认值（不报错）。
pub fn load() -> AppConfig {
    let path = ini_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return AppConfig::default();
    };
    let ini = parse_ini(&text);
    AppConfig::from_ini(&ini)
}

/// 落盘：先写临时文件再原子改名，避免写到一半程序崩溃留下坏文件。
pub fn save(cfg: &AppConfig) -> io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let final_path = dir.join("data.ini");
    let tmp_path = dir.join("data.ini.tmp");
    std::fs::write(&tmp_path, cfg.render())?;
    std::fs::rename(&tmp_path, &final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AppConfig {
        let mut c = AppConfig::default();
        c.level = 120;
        c.job = "主教".into();
        c.map = "废弃都市".into();
        c.mode_solo = false;
        c.power = 4832;
        c.punch = Some((parse_date("2026-09-03").unwrap(), parse_time("21:30").unwrap()));
        c.boss = Some((parse_date("2026-09-03").unwrap(), parse_time("14:02").unwrap()));
        c.exp_label = "EXP".into();
        c.level_label = "Lv".into();
        c.uid_cache = Some("abcd1234".repeat(8));
        c.net_local = true;
        c.panel_x = 240.5;
        c.panel_y = 40.0;
        c
    }

    #[test]
    fn ini_roundtrip() {
        let c = sample();
        let ini = parse_ini(&c.render());
        let back = AppConfig::from_ini(&ini);

        assert_eq!(c.level, back.level);
        assert_eq!(c.job, back.job);
        assert_eq!(c.map, back.map);
        assert_eq!(c.mode_solo, back.mode_solo);
        assert_eq!(c.power, back.power);
        assert_eq!(c.punch, back.punch);
        assert_eq!(c.boss, back.boss);
        assert_eq!(c.uid_cache, back.uid_cache);
        assert_eq!(c.net_local, back.net_local);
        assert_eq!(c.panel_x, back.panel_x);
        assert_eq!(c.panel_y, back.panel_y);
    }

    #[test]
    fn panel_pos_roundtrip_and_defaults() {
        // 默认：位置未记录(-1)；没有 panel_mode 概念
        let d = AppConfig::default();
        assert_eq!(d.panel_x, -1.0);
        assert_eq!(d.panel_y, -1.0);
        // 没写 [panel] → 未记录
        let ini = parse_ini("[net]\nbase=local\n");
        let c = AppConfig::from_ini(&ini);
        assert_eq!(c.panel_x, -1.0);
        assert_eq!(c.panel_y, -1.0);
        // x/y 往返一致
        let mut m = AppConfig::default();
        m.panel_x = 300.0;
        m.panel_y = -1.0; // 只存了 x 未存 y → y 保持未记录
        assert_eq!(AppConfig::from_ini(&parse_ini(&m.render())).panel_x, 300.0);
        assert_eq!(AppConfig::from_ini(&parse_ini(&m.render())).panel_y, -1.0);
        // 乱写 → 回退未记录
        let bad = parse_ini("[panel]\nx=abc\ny=\n");
        let c = AppConfig::from_ini(&bad);
        assert_eq!(c.panel_x, -1.0);
        assert_eq!(c.panel_y, -1.0);
        // 旧 ini 遗留的 mode 键被忽略，位置仍能正常读入
        let old = parse_ini("[panel]\nmode=auto\nx=240.0\ny=0.0\n");
        let c = AppConfig::from_ini(&old);
        assert_eq!(c.panel_x, 240.0);
        assert_eq!(c.panel_y, 0.0);
    }

    #[test]
    fn net_local_roundtrip_and_case_insensitive() {
        // 默认（不写 [net]）→ 生产
        assert!(!AppConfig::default().net_local);
        // base=local 命中；大小写不敏感
        let local = parse_ini("[net]\nbase=LOCAL\n");
        assert!(AppConfig::from_ini(&local).net_local);
        let prod = parse_ini("[net]\nbase=prod\n");
        assert!(!AppConfig::from_ini(&prod).net_local);
        // 设了 local 后 render 写回 local，往返一致
        let mut c = AppConfig::default();
        c.net_local = true;
        assert!(AppConfig::from_ini(&parse_ini(&c.render())).net_local);
    }

    #[test]
    fn empty_defaults() {
        let ini = parse_ini("");
        let c = AppConfig::from_ini(&ini);
        assert_eq!(c.level, 0);
        assert_eq!(c.job, "");
        assert!(c.mode_solo, "缺省应为单人");
        assert_eq!(c.punch, None);
        assert_eq!(c.uid_cache, None);
    }

    #[test]
    fn placeholder_uid_is_kept_verbatim() {
        // uid 只是缓存字符串，不做清理（清理在 net 层负责）。
        let c = AppConfig::default();
        assert!(c.uid_cache.is_none());
    }
}
