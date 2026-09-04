//! `data\` 目录下 JSON 数据表的加载与查询。
//!
//! - jobs.json : `[{group, jobs:[..]}]` 职业分组
//! - maps.json : `[{mapid, mapName, scene, mapDesc, street}]` 地图
//!
//! 曾经还用过 exp-table.json（各等级所需经验）：自采样改读全量整数 EXP 差分后不再需要等级表，
//! 已连同整条 exp-table 读取代码一起删除（见仓库清理记录）。
//!
//! **数据已内嵌**：两份表在编译时以 include_bytes 烤进 exe（release 单文件分发不必带 data/）；
//! 运行时仍优先读 exe 旁的 `data/<name>` 文件（便于不改代码就热更数据），找不到再退回内嵌那份。

use std::borrow::Cow;
use std::io;

use serde::Deserialize;

use crate::config::find_data_file;

/// 魔法师系 = group 精确等于该值（牧师等职业名里不含"法师"，不能靠名字猜）。
pub const MAGIC_GROUP: &str = "魔法师系";

/// 编译期内嵌的 data 表字节（release 单 exe 时的后备数据源）。
const JOBS_JSON: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/jobs.json"));
const MAPS_JSON: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/maps.json"));

#[derive(Debug, Clone, Deserialize)]
pub struct JobGroup {
    pub group: String,
    pub jobs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MapInfo {
    /// 地图表自带字段：当前只用到 `name`/`scene`；mapid/desc/street 保留给日后
    /// 服务端上报（mapid）与搜索辅助信息（street/desc）。
    #[allow(dead_code)]
    #[serde(rename = "mapid")]
    pub mapid: i64,
    #[serde(rename = "mapName")]
    pub name: String,
    pub scene: String,
    #[allow(dead_code)]
    #[serde(rename = "mapDesc", default)]
    pub desc: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub street: Option<String>,
}

/// 解析一份 data JSON：先读 exe 旁的磁盘文件（可覆盖/热更），找不到就用内嵌字节。
fn load_json<T: for<'de> Deserialize<'de>>(name: &str, embedded: &[u8]) -> io::Result<T> {
    let bytes: Cow<'_, [u8]> = match find_data_file(name) {
        Some(path) => std::fs::read(&path)?.into(),
        None => embedded.into(), // data/ 没放旁边 → 退回烤进 exe 的那份
    };
    serde_json::from_slice(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{name} JSON 解析失败: {e}"),
        )
    })
}

pub fn load_jobs() -> io::Result<Vec<JobGroup>> {
    load_json("jobs.json", JOBS_JSON)
}

pub fn load_maps() -> io::Result<Vec<MapInfo>> {
    load_json("maps.json", MAPS_JSON)
}

/// 找某个职业在 groups 中的位置 `(组下标, 组内下标)`。
pub fn find_job(groups: &[JobGroup], job: &str) -> Option<(usize, usize)> {
    groups.iter().enumerate().find_map(|(gi, g)| {
        g.jobs.iter().position(|j| j == job).map(|ji| (gi, ji))
    })
}

/// 只保留 `scene == "hunting"` 的打怪地图。
pub fn filter_hunting(maps: &[MapInfo]) -> Vec<MapInfo> {
    maps.iter().filter(|m| m.scene == "hunting").cloned().collect()
}

/// 地图搜索：子串匹配（大小写不敏感），并确保选中的名称是列表里真实存在的一个。
pub fn map_search<'a>(maps: &'a [MapInfo], query: &str) -> Vec<&'a MapInfo> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    maps.iter()
        .filter(|m| m.name.to_lowercase().contains(&q))
        .take(50)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_counts() {
        let groups = load_jobs().expect("jobs.json 应可加载");
        let all = load_maps().expect("maps.json 应可加载");

        assert_eq!(groups.len(), 5, "应有 5 个职业组");
        assert!(all.iter().any(|m| m.scene == "hunting"));
        assert_eq!(filter_hunting(&all).len(), 237, "hunting 地图应为 237");
    }

    #[test]
    fn magic_detection() {
        let groups = load_jobs().unwrap();
        // 反查职业下标（按组名判魔法师系：group == "魔法师系"）
        let (gi, ji) = find_job(&groups, "火毒法师").expect("应能反查到");
        assert_eq!(groups[gi].jobs[ji], "火毒法师");
        assert_eq!(groups[gi].group, "魔法师系");
        assert!(find_job(&groups, "不存在").is_none());
        // 组名判定直接可用（report.rs 用这个判“攻击力/魔法力”标签）
        assert!(groups.iter().any(|g| g.jobs.contains(&"牧师".to_owned()) && g.group == MAGIC_GROUP));
    }

    #[test]
    fn map_search_works() {
        let all = load_maps().unwrap();
        let hunting = filter_hunting(&all);
        // 数据里废都片区写的是"废都"，如 废都森林/废都北入口。
        let hits = map_search(&hunting, "废都");
        assert!(!hits.is_empty(), "搜索'废都'应有结果");
        for h in &hits {
            assert!(h.name.contains("废都"));
        }
        // 空查询 → 空结果；不存在的字 → 空结果
        assert!(map_search(&hunting, "").is_empty());
        assert!(map_search(&hunting, "不存在的图").is_empty());
    }

    #[test]
    fn embedded_fallback_parses() {
        // release 单 exe 不带 data/ 时读的是内嵌字节；这里直接验证烤进 exe 的两份表可解析、
        // 且与磁盘版口径一致（5 职业组 / 237 张打怪图）。
        let groups: Vec<JobGroup> = serde_json::from_slice(JOBS_JSON).expect("内嵌 jobs.json 应可解析");
        let all: Vec<MapInfo> = serde_json::from_slice(MAPS_JSON).expect("内嵌 maps.json 应可解析");
        assert_eq!(groups.len(), 5);
        assert_eq!(filter_hunting(&all).len(), 237);
    }
}
