# 冒险岛怀旧服经验记录器

Windows 无边框窄工具条(1360×36),用于记录一段练级/刷怪过程:

1. **输入 BAR**:等级 / 职业(经典二转 12 职业,5 系分组)/ 地图(全站 705 张,含金银岛/彩虹岛/隐藏地图/迷宫等,支持按名称/ID/区域搜索)/ 模式(组队/单人选项卡)/ 当前金币(单位:万)/ HP 药水(图标选种类+数量)/ MP 药水 / 当前 EXP(百分比 / 数值双模式)/ **开始记录** / **999打卡** / **神秘商人** → 开始计时
   - 等级与金币输入框点击默认全选,方便直接改数
   - 标签与输入左右排列,数字输入无增减按钮,药水数量点击自动全选
   - 地图搜索与药水图标选择通过**悬浮面板**(独立小窗,锚定在主条下方)完成,不改变主条窗口尺寸
   - **999打卡**:点击倒计时框弹出时分选择器(默认当前时分)→ 距上次打卡 24 小时的倒计时(绿色),到 0 转红色"待打卡"
   - **神秘商人**:6 小时周期循环倒计时,到 0 时振动并显示「已刷新」(内部周期照常继续),鼠标移到该框停止振动、恢复倒计时;点击输入**距下次刷新的剩余时分秒**(0:1:30 = 还剩1分30秒,最多 5:59:59)校准;两者均为时间戳基准,关闭程序时间照走
2. **计时器**:整条 BAR 变为 00:00:00 计时 + 停止记录
3. **结束 BAR**:等级 / 金币 / HP·MP 药水 / EXP(自动预填开始值)→ 提交数据
4. **成功动画**:提示已上传并展示本设备**分享链接**(服务端返参 `id` 拼接 `exp.html?id=<deviceId>`,本地/生产域名自动切换),立即前往 / 重新开始 按钮

- 同一电脑唯一 ID(注册表 MachineGuid)
- **上报服务端**:`POST /api/exp/report`,未打包(`npm start`)默认 `http://127.0.0.1:3001`,打包后默认 `https://mxd.zhuzhu.website`(环境变量 `EXP_API` 可覆盖);无鉴权头(服务端不校验密钥)
- 失败重试:退避 5s → 10s,网络错误/429 重试,400/403/413 直接失败;无论成败,payload 都存档到 `%APPDATA%\mxd-exp-recorder\records\` 兜底
- 上报字段按服务端校验兜底:gold/expGained 负值 clamp 为 0、levelsGained 0~100、durationSeconds 1~21600
- 上报 JSON 内置**收益报告** `profit`:经验/金币增量按比例折算到 1 小时标准单位(`expPerHour` / `goldPerHour`);药水折价按 1 回蓝 = 2 金币、1 回血 = 1 金币,回血/回蓝分开上报(`potionHpValue` / `potionMpValue`,全满/百分比恢复无法界定则不计算)
- 数据全部内置快照(不依赖网络):经验表 199 级区间、全站地图 705 张(含所属区域)、药水 58 种(含 HP/MP 恢复值,已按名称+数值去重)、职业 12 个
- 工具条支持按住空白处拖动;右上角 ✕ 关闭

## 使用

```bash
npm install            # 首次
npm start              # 开发运行
npm run dist           # 打包 → out\mxd-exp-recorder.exe (约 92MB, portable 单文件)
```

> 国内网络下 electron 二进制下载失败时:
> `set ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/` 后再执行 npm install / dist。

## 开发

| 命令 | 说明 |
| --- | --- |
| `npm run fetch-data` | 从 mxdc.dvg.cn 重新抓取快照到 `data\*.json`(站点更新后重新打包即可) |
| `npx electron scripts/dev-verify.js` | DOM 自动化走查 4 个状态,34 项断言 |
| `npx electron scripts/dev-shot.js` | 自动截图各状态到 `shots\` |
| `npx electron . --smoke-test` | 冒烟:写一条测试记录后退出 |

## 数据源(快照自冒险岛怀旧服小册子)

- 经验表:`experience-calculator.js` 内嵌 `EXP_TABLE`(下标 0 = 1→2 级,199 项,200 级满级)
- 地图:`/api/map-list.php`(全量分页,附带 streetName 所属区域)
- 药水:`/api/item_list.php?category=ItemDetailCategory_Consume/Potion/Potion`(解析 desc 中的 HP/MP 恢复数值,排除纯 BUFF 食品)
- 药水图标:`/dbsource/icon/item/{itemid}.png` 逐张下载到 `data\potion-icons\`(缺失时 UI 回退自绘图标)
- 职业:站点「1—70级职业加点」15 条路线合并为经典二转 12 职业
