# 架构

## 总览

```
┌─ Tauri 进程（Rust，主线程 = 事件循环）────────────────────┐
│  main 窗口    字幕控制台（App.tsx：稿件/设置/预览）         │
│  overlay 窗口 字幕条（overlay.tsx：播放状态机自治）          │
│  preview 窗口 预览排练（preview.tsx：全屏翻页图）           │
│  COM 轮询线程（独立 STA）：250ms 拉 PowerPoint 放映状态      │
└──────────────────────────────────────────────────────┘
        ↑↓ IPC（invoke 命令 + emit 事件）         ↑ WebView2
```

- 目录：`app/src`（前端 React/TS）、`app/src-tauri/src`（Rust：`lib.rs` 窗口/命令/设置，`ppt.rs` COM 与窗口探测，`script.rs` 稿件解析）
- 多页入口：`index.html`（主）/ `overlay.html` / `preview.html`，vite 构建

## 窗口与职责

| 窗口 | 职责 | 特点 |
|---|---|---|
| main | 稿件编辑、设置、预览、控制按钮；持有 settings 的**前端权威状态** | 普通窗口 |
| overlay | 播放状态机自治：滚动引擎（rAF）、逐条推进、暂停、自报高度（R18） | 透明、置顶、可穿透、可拖动 |
| preview | 预览排练：全屏放导入的页图，真实放映开始时自动让位关闭 | 无边框最大化 |

## 数据流（单向）

1. **settings**：前端 `Settings` state（camelCase）↔ Rust `Settings` struct（snake_case，`types.ts` 逐字段映射）。任何改动 → `push_style`/`apply_style` → `sync_window_flags`（置顶/穿透/布局）+ `place_overlay`（落位）→ `emit("style")` 广播给 overlay
2. **页字幕**：`push_page`（页号 + 条目 + settings）→ overlay 收 `page` 事件开始播放 → `emit("playback-status")` 回报主界面
3. **PPT 状态**：轮询线程 → `emit("ppt-state" | "ppt-show-end" | "ppt-detached")` → 主界面跟随翻页 / 自动隐藏 / 断开清理
4. **位置回写**：字幕条被用户拖动 → overlay `onMoved`（防抖 250ms，且仅在用户按下拖动区后才上报，防程序化落位回写成环）→ `emit("bar-moved")` → 主界面写回 settings.posX/posY → 保存 + 重新落位

## 落位公式（关键不变量）

字幕条几何的**唯一权威**在 Rust `lib.rs::bar_rect`：

- 宽度 = 屏宽 × width_ratio；高度 = `overlay_height`（内边距×2 + 行数×字号×1.5 + 英文行额外）或 overlay 自报高度（`LAST_BAR_HEIGHT`，长高时**底部锚定**重新落位，保证不出屏）
- 默认位置 = 水平居中 + 距底 bottom_ratio；`pos_x/pos_y`（比例 0..1）存在时 = 左上角直接落位并钳制在屏内
- 贴边布局走 `ppt.rs::apply_dock` / `dock_geometry`：目标窗（放映窗优先，其次预览窗）让位 + 字幕条占让位条

前端预览（`App.tsx` + `subbar.tsx::barContentHeight`）**逐行复刻**同一公式：舞台按目标屏（`settings.screen` 选屏逻辑同 `target_monitor`）宽高比缩放，`k = 舞台宽 × 屏 scale / 屏物理宽`，条窗口矩形 = `bar_rect` 的比例映射。**改动任何一端的几何公式必须同步另一端**，否则预览与实际播放不一致（硬伤①）。

渲染一致性靠 `subbar.tsx::BarView` 共享组件：真实 overlay 与预览用同一份标记/样式，scale=1（真实）或 k（预览）。

## COM 轮询（ppt.rs）

独立 STA 线程（`COINIT_APARTMENTTHREADED`），永不占主线程：

- `GetActiveObject` 附着运行中的 PowerPoint，失败 `CoCreateInstance` 兜底；实例缓存
- 250ms 周期：`SlideShowWindows(1).View.Slide.SlideIndex` 读放映页；`EnumWindows` 找 `screenClass`（放映窗，取所在屏矩形）/ `PPTFrameClass`（主窗存活检查）
- 断开挂载：按路径匹配关掉我们挂的 deck、清 `LAST_PPTX`/`PPT_SHOW`、发 `ppt-detached`
- 最后页再 Next 时 `View.Slide` 抛 Invalid request → 返回 (true, None) 保持上一页码

## 播放状态机（overlay.tsx）

模式矩阵（PRD 6.3）：滚动模式只响应滚动类控制（F9/F8 无效）；逐条模式反之。模式切换即重置播放状态。滚动用 rAF 引擎按 `scrollSpeed` 行/秒推进，`stayAtEnd` 时钳制在末行；内容折行超基础行数时自报高度（R18，封顶 5 行），Rust 端按全高底部锚定落位。

## 存储

- `store.json`（`%APPDATA%\com.luna-jiang.windows-subtitle\`）：settings + script，前端 600ms 防抖保存；`load_store` 对空稿填示例稿
- 导入预览图：开发模式在仓库 `app/imports/`（release 在应用数据目录），断开 PPT 时整目录清理
- WebView2 数据目录：开发模式强制指到仓库 `.webview2-data/`（绕开沙箱 AppData\Local 写入限制），可用环境变量 `DESKTOP_SUBTITLE_DATA_DIR` 覆盖

## 构建与调试

- 前端类型检查：`npx tsc --noEmit`（app/）；Rust：`cargo build --no-default-features`（src-tauri/，当前 0 warning）
- WebView2 远程调试（看前端 console / 直接调 invoke）：设 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` 启动，CDP 连 `http://127.0.0.1:9333/json`
- 自动化参数：`--dev-show-overlay`、`--dev-selftest`、`--dev-import <pptx>`（仅 debug 构建）
