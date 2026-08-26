// 一次性迁移:把 Tauri 版的 settings.ini 转回 Electron 版读取的 settings.json
// 用法:node scripts/ini-to-json.mjs [ini路径] [json路径]
import { readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const iniPath = process.argv[2] || join(process.env.APPDATA || homedir(), 'mxd-exp-recorder', 'settings.ini');
const jsonPath = process.argv[3] || join(process.env.APPDATA || homedir(), 'mxd-exp-recorder', 'settings.json');

const text = readFileSync(iniPath, 'utf-8');
const out = {};
let windowArr = [0, 0];
let potions = {};
let map = {};
let section = '';
const str = (v) => v.trim();
const num = (v, d = 0) => { const n = Number(str(v)); return Number.isFinite(n) ? n : d; };

for (const line of text.split(/\r?\n/)) {
  const t = str(line);
  if (!t || t.startsWith(';') || t.startsWith('#')) continue;
  if (t.startsWith('[') && t.endsWith(']')) { section = t.slice(1, -1); continue; }
  const eq = t.indexOf('=');
  if (eq < 0) continue;
  const k = str(t.slice(0, eq));
  const v = str(t.slice(eq + 1));
  switch (section) {
    case 'window':
      if (k === 'x') windowArr[0] = num(v);
      if (k === 'y') windowArr[1] = num(v);
      break;
    case 'player':
      if (k === 'level' || k === 'outLevel') out[k] = num(v, 1);
      else out[k] = v;
      break;
    case 'potions':
      potions[k] = num(v);
      break;
    case 'checkin':
      out[k] = num(v);
      break;
    case 'map':
      if (k === 'mapid') map[k] = num(v);
      else map[k] = v;
      break;
  }
}
out.window = windowArr;
out.potions = potions;
out.map = map;
writeFileSync(jsonPath, JSON.stringify(out, null, 2), 'utf-8');
console.log('已写出', jsonPath);
console.log(JSON.stringify(out));
