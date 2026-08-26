// 一次性数据快照生成脚本:从冒险岛怀旧服小册子 (mxdc.dvg.cn) 抓取
// 经验表 / 金银岛地图 / 药水列表,写入 data/*.json 随 exe 打包。
// 用法: npm run fetch-data
import { writeFile, mkdir, readdir, unlink } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const OUT = join(ROOT, 'data');
const UA = { 'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' };

async function getJSON(url) {
  const res = await fetch(url, { headers: UA });
  if (!res.ok) throw new Error(`${res.status} ${url}`);
  return res.json();
}

async function getText(url) {
  const res = await fetch(url, { headers: UA });
  if (!res.ok) throw new Error(`${res.status} ${url}`);
  return res.text();
}

async function getBuffer(url) {
  const res = await fetch(url, { headers: UA });
  if (!res.ok) throw new Error(`${res.status} ${url}`);
  return Buffer.from(await res.arrayBuffer());
}

// ---------- 1. 经验表:从经验计算器 JS 中提取 EXP_TABLE ----------
async function fetchExpTable() {
  const js = await getText('https://mxdc.dvg.cn/public/assets/js/experience-calculator.js');
  const m = js.match(/const EXP_TABLE = Object\.freeze\(\[([\s\S]*?)\]\);/);
  if (!m) throw new Error('EXP_TABLE 未在 experience-calculator.js 中找到');
  const perLevel = m[1].split(',').map((s) => s.trim()).filter(Boolean).map((s) => parseInt(s, 10));
  if (perLevel.length !== 199 || perLevel.some((n) => !Number.isFinite(n))) {
    throw new Error(`EXP_TABLE 解析异常: ${perLevel.length} 项`);
  }
  // cumulative[i] = 从 1 级升到 i+1 级所需的累计经验 (cumulative[0]=0)
  const cumulative = [0];
  for (const v of perLevel) cumulative.push(cumulative[cumulative.length - 1] + v);
  return { maxLevel: 200, perLevel, cumulative };
}

// ---------- 2. 全站地图(不分街道,按 meta.totalPages 全量分页) ----------
async function fetchMaps() {
  const first = await getJSON(
    'https://mxdc.dvg.cn/api/map-list.php?page=1&pageSize=100&sortKey=mapid&sortDir=asc&lng=zh-CN'
  );
  const totalPages = first.meta.totalPages;
  const items = [];
  for (let page = 1; page <= totalPages; page++) {
    const d = page === 1
      ? first
      : await getJSON(
          `https://mxdc.dvg.cn/api/map-list.php?page=${page}&pageSize=100&sortKey=mapid&sortDir=asc&lng=zh-CN`
        );
    for (const it of d.items) {
      items.push({
        mapid: it.mapid,
        mapName: it.mapName,
        scene: it.scene || '',
        mapDesc: it.mapDesc || '',
        street: it.streetName || '', // 所属区域(金银岛/彩虹岛/隐藏地图/迷宫…)
      });
    }
  }
  if (items.length !== first.meta.total) {
    console.warn(`警告: 地图数量 ${items.length} ≠ ${first.meta.total}`);
  }
  items.sort((a, b) => a.mapid - b.mapid);
  return items;
}

// ---------- 3. 药水:解析 desc 中的 HP/MP 恢复数值 ----------
function parseRecovery(desc) {
  const d = (desc || '').replace(/\\n/g, ' ');
  const r = { hp: 0, mp: 0, hpPct: 0, mpPct: 0, full: false };

  if (/恢复血量、魔量全满|血量，魔量全部恢复|血量、魔量全部恢复|全部恢复|全满/.test(d)) {
    r.full = true;
    return r;
  }
  let m;
  if ((m = d.match(/恢复血量约(\d+)%[，,]?[^。]*魔量约(\d+)%/) || d.match(/恢复血量约(\d+)%，魔量约(\d+)%/))) {
    r.hpPct = +m[1]; r.mpPct = +m[2]; return r;
  }
  if ((m = d.match(/可恢复(\d+)%的血量与魔量/) || d.match(/血量、魔量恢复约(\d+)%/) || d.match(/使血量、魔量恢复约(\d+)%/))) {
    r.hpPct = +m[1]; r.mpPct = +m[1]; return r;
  }
  if ((m = d.match(/血量，魔量各恢复(\d+)/))) { r.hp = +m[1]; r.mp = +m[1]; return r; }
  if ((m = d.match(/恢复血量约(\d+)[，,]?\s*魔量约(\d+)/))) { r.hp = +m[1]; r.mp = +m[2]; return r; }
  if ((m = d.match(/恢复血量(\d+)[，,]魔量(\d+)/))) { r.hp = +m[1]; r.mp = +m[2]; return r; }
  if ((m = d.match(/约恢复(\d+)血量/))) { r.hp = +m[1]; return r; }
  if ((m = d.match(/恢复血量约(\d+)/) || d.match(/恢复血量\s*(\d+)/) || d.match(/血量恢复约?(\d+)/) || d.match(/血量可恢复(\d+)/))) { r.hp = +m[1]; }
  if ((m = d.match(/恢复魔量约(\d+)/) || d.match(/魔量恢复约?(\d+)/) || d.match(/魔量可恢复(\d+)/) || d.match(/恢复魔量\s*(\d+)/))) { r.mp = +m[1]; }
  return r;
}

async function fetchPotions() {
  const url =
    'https://mxdc.dvg.cn/api/item_list.php?category=ItemDetailCategory_Consume%2FPotion%2FPotion' +
    '&catalog=item&sort=itemid&dir=asc&page=1&page_size=100';
  const d = await getJSON(url);
  const rows = d.rows || [];
  const potions = [];
  const seenKeys = new Set();
  for (const it of rows) {
    const rec = parseRecovery(it.desc);
    if (!rec.full && !rec.hp && !rec.mp && !rec.hpPct && !rec.mpPct) continue; // 纯 BUFF 食品等
    // 站点会把同一条目以多个 itemid 重复收录(名称+数值完全相同),只保留 itemid 最小的
    const key = `${it.name}|${rec.hp}|${rec.mp}|${rec.hpPct}|${rec.mpPct}|${rec.full}`;
    if (seenKeys.has(key)) continue;
    seenKeys.add(key);
    potions.push({
      itemid: it.itemid,
      name: it.name,
      icon: it.icon || `/dbsource/icon/item/${it.itemid}.png`,
      hp: rec.hp,
      mp: rec.mp,
      hpPct: rec.hpPct,
      mpPct: rec.mpPct,
      full: rec.full,
      price: it.price ?? 0,
    });
  }
  return potions;
}

// ---------- 4. 药水图标:逐张下载到 data/potion-icons/{itemid}.png ----------
async function fetchPotionIcons(potions) {
  const ICON_DIR = join(OUT, 'potion-icons');
  await mkdir(ICON_DIR, { recursive: true });
  let ok = 0;
  // 每批 10 个,避免瞬间并发过高
  for (let i = 0; i < potions.length; i += 10) {
    const batch = potions.slice(i, i + 10).map(async (p) => {
      try {
        const buf = await getBuffer(`https://mxdc.dvg.cn${p.icon}`);
        await writeFile(join(ICON_DIR, `${p.itemid}.png`), buf);
        ok++;
      } catch (e) {
        console.warn(`图标下载失败 ${p.itemid} (${p.name}): ${e.message}`);
      }
    });
    await Promise.all(batch);
  }
  // 清理已去重药水遗留的旧图标
  const wanted = new Set(potions.map((p) => `${p.itemid}.png`));
  const existing = await readdir(ICON_DIR);
  const stale = existing.filter((f) => f.endsWith('.png') && !wanted.has(f));
  await Promise.all(stale.map((f) => unlink(join(ICON_DIR, f))));
  if (stale.length) console.log(`清理过期图标 ${stale.length} 张`);
  return ok;
}

// ---------- 5. 职业:经典二转 12 职业,按 5 大系分组 ----------
const JOBS = [
  { group: '战士系', jobs: ['剑客', '准骑士', '枪骑士'] },
  { group: '魔法师系', jobs: ['牧师', '火毒法师', '冰雷法师'] },
  { group: '弓箭手系', jobs: ['猎人', '弩弓手'] },
  { group: '飞侠系', jobs: ['刺客', '侠客'] },
  { group: '海盗系', jobs: ['拳手', '火枪手'] },
];

// ---------- 主流程 ----------
async function main() {
  await mkdir(OUT, { recursive: true });
  const [exp, maps, potions] = await Promise.all([fetchExpTable(), fetchMaps(), fetchPotions()]);
  await Promise.all([
    writeFile(join(OUT, 'exp-table.json'), JSON.stringify(exp), 'utf-8'),
    writeFile(join(OUT, 'maps.json'), JSON.stringify(maps), 'utf-8'),
    writeFile(join(OUT, 'potions.json'), JSON.stringify(potions), 'utf-8'),
    writeFile(join(OUT, 'jobs.json'), JSON.stringify(JOBS), 'utf-8'),
  ]);
  const iconOk = await fetchPotionIcons(potions);
  const hpOnly = potions.filter((p) => p.full || p.hp || p.hpPct).length;
  const mpOnly = potions.filter((p) => p.full || p.mp || p.mpPct).length;
  console.log(`经验表: ${exp.perLevel.length} 级区间 (满级 ${exp.maxLevel})`);
  console.log(`地图: ${maps.length} 张(全站,含所属区域)`);
  console.log(`药水: ${potions.length} 种 (可回血 ${hpOnly} / 可回魔 ${mpOnly}), 图标 ${iconOk}/${potions.length}`);
  console.log(`职业: ${JOBS.reduce((n, g) => n + g.jobs.length, 0)} 个, ${JOBS.length} 系`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
