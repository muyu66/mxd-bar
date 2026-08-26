// 构建后将单文件便携 exe 复制到 out/(与旧 Electron 版产物目录一致)
// 路径基于脚本自身位置解析,与执行时的 cwd 无关
import { copyFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = new URL('..', import.meta.url); // 项目根
const src = fileURLToPath(new URL('src-tauri/target/release/mxd-bar.exe', root));
const outDir = fileURLToPath(new URL('out', root));

mkdirSync(outDir, { recursive: true });
copyFileSync(src, outDir + '/mxd-bar.exe');
console.log('已输出 ' + outDir + '/mxd-bar.exe');
