import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  Cue,
  DEFAULT_SETTINGS,
  Settings,
  cuesOfPage,
  fromRustSettings,
  normalizeDraft,
  parseScript,
  toRustSettings,
} from "./types";
import { BarView, barContentHeight } from "./subbar";
import "./styles.css";

type MonitorInfo = {
  index: number;
  name: string;
  width: number;
  height: number;
  scale: number;
  primary: boolean;
};

const HOTKEYS: [string, string][] = [
  ["Ctrl + Alt + 空格", "暂停 / 继续滚动（仅滚动模式）"],
  ["Ctrl + Alt + ↑ / ↓", "滚动模式：手动推 / 退一段"],
  ["Ctrl + Alt + → / ←", "下一页 / 上一页（手动翻页兜底）"],
  ["Ctrl + Alt + R", "回到本页开头重滚"],
  ["F9 / F8", "下一条 / 上一条（仅逐条模式）"],
  ["F7", "显示 / 隐藏字幕条"],
  ["Ctrl + Alt + P", "显示 / 隐藏这个控制台"],
  ["Ctrl + Alt + X", "临时切换点击穿透（默认关，字幕条可直接拖动）"],
  ["Ctrl + Alt + = / −", "放大 / 缩小字号"],
  ["Ctrl + Alt + Q", "退出程序"],
];

export default function App() {
  const [script, setScript] = useState("");
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [slide, setSlide] = useState(1);
  const [overlayShown, setOverlayShown] = useState(false);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [saved, setSaved] = useState<"idle" | "saving" | "done" | "error">("idle");
  const [ready, setReady] = useState(false);
  const [tab, setTab] = useState<"script" | "settings">("script");
  const [liveTick, setLiveTick] = useState(0); // overlay 加载完成后 +1，触发首次 push_page
  const [pbStatus, setPbStatus] = useState<{ index: number; total: number; paused: boolean; atEnd: boolean } | null>(null);
  const [pptLive, setPptLive] = useState(false); // PPT 放映中（COM 轮询）
  const [notice, setNotice] = useState(""); // 导入等操作的提示条
  const [slideImg, setSlideImg] = useState<string | null>(null); // 当前页预览图
  const [imgTick, setImgTick] = useState(0); // 导入后刷新预览图
  const [pptImported, setPptImported] = useState(false); // 本次运行是否已导入 PPT（按钮显隐）
  const [stageWrapW, setStageWrapW] = useState(0); // 预览包裹层实测宽（ResizeObserver，跨 tab 用 callback ref 重连）
  const [wrapEl, setWrapEl] = useState<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const settingsRef = useRef(settings);
  const dragRef = useRef<{ dx: number; dy: number } | null>(null);

  settingsRef.current = settings;

  const cues: Cue[] = useMemo(() => parseScript(script), [script]);
  const pageIdx = useMemo(() => cuesOfPage(cues, slide), [cues, slide]);
  const pages = useMemo(() => {
    const set = new Set<number>();
    cues.forEach((c) => set.add(c.page));
    return Array.from(set).sort((a, b) => a - b);
  }, [cues]);

  // ---------- 初始化 ----------
  useEffect(() => {
    (async () => {
      try {
        const store = await invoke<any>("load_store");
        setScript(store.script ?? "");
        setSettings(fromRustSettings(store.settings ?? {}));
        const mons = await invoke<MonitorInfo[]>("list_monitors");
        setMonitors(mons);
        setSlide(1);
      } catch (e) {
        console.error(e);
      } finally {
        setReady(true);
      }
    })();
  }, []);

  // ---------- 预览图：当前页幻灯片背景（R11，未导入则占位） ----------
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const d = await invoke<string | null>("read_preview_image", { page: slide });
        if (alive) setSlideImg(d ?? null);
      } catch {
        if (alive) setSlideImg(null);
      }
    })();
    return () => {
      alive = false;
    };
  }, [slide, imgTick]);

  // ---------- 预览包裹层宽度实测（预览=真实效果的比例换算基准） ----------
  // callback ref：切 tab 卸载/挂载后 RO 自动重连（初始挂载在设置页时 useRef 为 null 会失联）
  useEffect(() => {
    if (!wrapEl) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? 0;
      setStageWrapW(w);
    });
    ro.observe(wrapEl);
    return () => ro.disconnect();
  }, [wrapEl]);

  // ---------- 保存（防抖） ----------
  useEffect(() => {
    if (!ready) return;
    setSaved("saving");
    const t = setTimeout(async () => {
      try {
        await invoke("save_store", {
          store: { settings: toRustSettings(settings), script },
        });
        setSaved("done");
      } catch (e) {
        console.error(e);
        setSaved("error");
      }
    }, 600);
    return () => clearTimeout(t);
  }, [script, settings, ready]);

  // ---------- 推送当前页字幕段 / 样式（播放状态机在 overlay，PRD 6.2） ----------
  useEffect(() => {
    if (!ready || !overlayShown) return;
    const list = cuesOfPage(cues, slide).map((i) => ({
      zh: cues[i].zh,
      en: cues[i].en ? cues[i].en : null,
    }));
    invoke("push_page", {
      page: slide,
      cues: list,
      settings: toRustSettings(settingsRef.current),
    }).catch(console.error);
  }, [slide, cues, ready, overlayShown, liveTick]);

  useEffect(() => {
    if (!ready) return;
    invoke("push_style", { settings: toRustSettings(settings) }).catch(console.error);
  }, [settings, ready]);

  // ---------- 全局热键（播放类热键由 overlay 自己处理，这里只管应用级） ----------
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    const reg = async () => {
      unlisteners.push(
        await listen("hk:toggle", () => toggleOverlay()),
        await listen("hk:panel", () => invoke("toggle_console").catch(() => {})),
        await listen("hk:font-up", () => bumpFont(2)),
        await listen("hk:font-down", () => bumpFont(-2)),
        await listen("hk:quit", () => invoke("quit_app").catch(() => {})),
        await listen("hk:page-next", () => setSlide((v) => Math.max(1, v + 1))),
        await listen("hk:page-prev", () => setSlide((v) => Math.max(1, v - 1))),
        await listen("hk:passthrough", () =>
          setSettings((s) => ({ ...s, clickThrough: !s.clickThrough }))
        ),
        await listen("overlay-ready", () => setLiveTick((t) => t + 1)),
        await listen<any>("playback-status", (e) => setPbStatus(e.payload)),
        // PPT COM 轮询（R4 跟随翻页）：followPpt 开着才跟
        await listen<any>("ppt-state", (e) => {
          const { showing, slide } = e.payload;
          setPptLive(Boolean(showing));
          if (showing) {
            // 真实放映开始：预览排练窗自动让位（PRD 6.4）
            invoke("close_preview").catch(() => {});
            if (slide && settingsRef.current.followPpt) {
              setSlide(Number(slide));
              applySettings(); // 顺带重算 overlay 位置（跟随放映屏 R12）
            }
          }
        }),
        await listen("ppt-show-end", () => {
          // 放映结束：字幕条自动隐藏（PRD 边界行为），F7 可再显示
          setPptLive(false);
          invoke("hide_overlay").catch(() => {});
          setOverlayShown(false);
        }),
        await listen("ppt-detached", () => {
          setPptLive(false);
          setPptImported(false);
          invoke("hide_overlay").catch(() => {});
          setOverlayShown(false);
          setImgTick((t) => t + 1); // 预览图已删，刷新成占位
          setNotice("已断开 PPT 挂载（deck 已关、预览图已清理），可导入其他 PPT");
        }),
        // 字幕条上直接拖动（overlay 窗口 OS 拖拽）→ 位置回写，预览与保存跟着同步（硬伤①）
        await listen<any>("bar-moved", (e) => {
          const { x, y } = e.payload;
          setSettings((s) => ({
            ...s,
            posX: Math.max(0, Math.min(1, Number(x))),
            posY: Math.max(0, Math.min(1, Number(y))),
          }));
        })
      );
    };
    reg();
    return () => unlisteners.forEach((u) => u());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function sendCtrl(kind: string, arg = 0) {
    invoke("send_ctrl", { kind, arg }).catch((e) => setNotice(`控制命令出错（${kind}）：${e}`));
  }

  function applySettings() {
    invoke("apply_style", { settings: toRustSettings(settingsRef.current) }).catch(console.error);
  }

  // ---------- 导入（R9 骨架+预览图 / R10 译稿） ----------
  async function importPpt() {
    try {
      const path = await openFileDialog({
        multiple: false,
        filters: [{ name: "PowerPoint", extensions: ["pptx"] }],
      });
      if (!path || typeof path !== "string") return;
      setNotice("正在导入 PPT（读页数/标题 + 导出预览图）…");
      const res = await invoke<any>("import_pptx", { path });
      setScript(res.script);
      setPptImported(true);
      setImgTick((t) => t + 1);
      setNotice(`已导入 ${res.pages} 页（内容取自演讲者备注），预览图在 ${res.dir}`);
    } catch (e) {
      console.error(e);
      setNotice(`导入失败：${e}`);
    }
  }

  async function importDraft() {
    try {
      const path = await openFileDialog({
        multiple: false,
        filters: [{ name: "译稿", extensions: ["md", "txt"] }],
      });
      if (!path || typeof path !== "string") return;
      const text = await invoke<string>("read_text_file", { path });
      setScript(normalizeDraft(text));
      setNotice("译稿已导入（## P6 格式已自动转为 === N ===）");
    } catch (e) {
      console.error(e);
      setNotice(`导入失败：${e}`);
    }
  }

  function pageStep(dir: number) {
    setSlide((v) => Math.max(1, v + dir));
  }

  function bumpFont(delta: number) {
    setSettings((s) => ({ ...s, fontSize: Math.max(20, Math.min(96, s.fontSize + delta)) }));
  }

  async function toggleOverlay() {
    try {
      const shown = await invoke<boolean>("overlay_visible");
      if (shown) {
        await invoke("hide_overlay");
        setOverlayShown(false);
      } else {
        await invoke("show_overlay", { settings: toRustSettings(settingsRef.current) });
        setOverlayShown(true);
      }
    } catch (e) {
      console.error(e);
      setNotice(`显示/隐藏字幕条出错：${e}`);
    }
  }

  function goSlide(next: number) {
    setSlide(Math.max(1, next));
  }

  // ---------- 预览几何：与 lib.rs bar_rect/target_monitor 同一选屏与公式（硬伤①②） ----------
  // 字幕条将落在哪块屏（screen 设置同 target_monitor：auto → 主屏），比例基准就用它
  const targetMon = useMemo(() => {
    const pick = settings.screen.trim();
    if (pick && !/^(auto|ppt)$/i.test(pick)) {
      const idx = Number(pick);
      if (Number.isFinite(idx)) {
        const m = monitors.find((mm) => mm.index === idx);
        if (m) return m;
      }
    }
    return monitors.find((m) => m.primary) ?? monitors[0];
  }, [monitors, settings.screen]);
  const monW = targetMon?.width ?? 1920;
  const monH = targetMon?.height ?? 1080;
  const monScale = targetMon?.scale ?? 1;
  const aspect = monW / monH;
  // 舞台尺寸：宽度 ≤ 容器宽，高度 ≤ 上限（全宽预览太占竖向空间）；按屏幕比例锁定
  const stageMaxH = Math.min(300, Math.max(180, Math.round(window.innerHeight * 0.32)));
  const stageW = stageWrapW > 0 ? Math.min(stageWrapW, Math.round(stageMaxH * aspect)) : 0;
  const stageH = stageW > 0 ? Math.round(stageW / aspect) : Math.round((stageMaxH * 16) / 9);
  // 字号等是逻辑像素：真实占屏比例 = 逻辑值×屏scale / 屏物理宽；预览同比例缩放
  const k = stageW > 0 ? (stageW * monScale) / monW : 0.3;
  const barW = settings.widthRatio * stageW;
  const barH = barContentHeight(settings) * k;
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
  // 浮层模式的字幕条窗口矩形（= 真实 overlay 窗口在屏上的落位比例）
  const floatRect = (() => {
    const left =
      settings.posX != null
        ? clamp(settings.posX * stageW, 0, stageW - barW)
        : (stageW - barW) / 2;
    const top =
      settings.posY != null
        ? clamp(settings.posY * stageH, 0, stageH - barH)
        : stageH * (1 - settings.bottomRatio) - barH;
    return { left, top, width: barW, height: barH };
  })();
  // 贴边模式的字幕条矩形 = 整条贴边（与 ppt.rs apply_dock/dock_geometry 对应）
  const dockRect = (() => {
    const r = clamp(settings.dockRatio, 0.08, 0.5);
    switch (settings.dockSide) {
      case "bottom":
        return { left: 0, top: stageH * (1 - r), width: stageW, height: stageH * r };
      case "left":
        return { left: 0, top: 0, width: stageW * r, height: stageH };
      case "right":
        return { left: stageW * (1 - r), top: 0, width: stageW * r, height: stageH };
      default: // top
        return { left: 0, top: 0, width: stageW, height: stageH * r };
    }
  })();
  const winRect = settings.layout === "dock" ? dockRect : floatRect;
  // 放映区矩形（dock 时被缩让位的那块；overlay 时=整个舞台）
  const slideRect = (() => {
    if (settings.layout !== "dock") return { left: 0, top: 0, width: stageW, height: stageH };
    const r = clamp(settings.dockRatio, 0.08, 0.5);
    switch (settings.dockSide) {
      case "bottom":
        return { left: 0, top: 0, width: stageW, height: stageH * (1 - r) };
      case "left":
        return { left: stageW * r, top: 0, width: stageW * (1 - r), height: stageH };
      case "right":
        return { left: 0, top: 0, width: stageW * (1 - r), height: stageH };
      default:
        return { left: 0, top: stageH * r, width: stageW, height: stageH * (1 - r) };
    }
  })();

  // ---------- 预览里拖动字幕条（硬伤①：拖完的位置即真实播放位置） ----------
  function onBarPointerDown(e: React.PointerEvent) {
    if (settings.layout !== "overlay" || !stageRef.current) return;
    const st = stageRef.current.getBoundingClientRect();
    dragRef.current = {
      dx: e.clientX - st.left - winRect.left,
      dy: e.clientY - st.top - winRect.top,
    };
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    setDragging(true);
  }
  function onBarPointerMove(e: React.PointerEvent) {
    if (!dragRef.current || !stageRef.current || stageW === 0) return;
    const st = stageRef.current.getBoundingClientRect();
    const nx = e.clientX - st.left - dragRef.current.dx;
    const ny = e.clientY - st.top - dragRef.current.dy;
    // 钳制在屏内（与 lib.rs bar_rect 一致：顶左不越过 屏宽-条宽 / 屏高-条高）
    setSettings((s) => ({
      ...s,
      posX: clamp(nx / stageW, 0, Math.max(0, 1 - barW / stageW)),
      posY: clamp(ny / stageH, 0, Math.max(0, 1 - barH / stageH)),
    }));
  }
  function onBarPointerUp() {
    dragRef.current = null;
    setDragging(false);
    // 拖完立即落位真实字幕条（push_style effect 也会触发，这里兜底一次）
    applySettings();
  }

  function resetBarPos() {
    setSettings((s) => ({ ...s, posX: null, posY: null }));
  }

  const totalPages = cues.length ? Math.max(...cues.map((c) => c.pageEnd)) : 1;
  const pageCueList = pageIdx.map((i) => ({ zh: cues[i].zh, en: cues[i].en || null }));
  const posCustom = settings.posX != null || settings.posY != null;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          字幕控制台
          <span className="brand-sub">Windows Subtitle</span>
        </div>
        <div className="top-status">
          <span className={`dot ${overlayShown ? "on" : "off"}`} />
          {overlayShown ? "字幕条已显示" : "字幕条未显示"}
          {pptLive ? (
            <>
              <span className="sep">·</span>
              PPT 放映中
            </>
          ) : null}
          <span className="sep">·</span>
          {cues.length} 条 / {totalPages} 页
          <span className="sep">·</span>
          {saved === "saving" ? "保存中…" : saved === "error" ? "保存失败" : "已保存"}
        </div>
        <div className="top-actions">
          {/* 按钮显隐：未导入只给「导入 PPT」；导入后换成「打开/断开」；断开后回到「导入」 */}
          {!pptImported ? (
            <button className="btn ghost" onClick={importPpt}>
              导入 PPT
            </button>
          ) : (
            <>
              <button
                className="btn ghost"
                onClick={() => invoke("open_last_ppt").catch((e) => setNotice(`打开 PPT 出错：${e}`))}
              >
                打开 PPT
              </button>
              <button
                className="btn ghost"
                onClick={() => invoke("detach_ppt").catch(() => {})}
                title="关掉软件挂载的 PPT、清理预览图并停止跟随，方便换下一个"
              >
                断开 PPT
              </button>
            </>
          )}
          <button className="btn ghost" onClick={importDraft}>
            导入译稿
          </button>
          <button
            className="btn"
            onClick={async () => {
              try {
                await invoke("open_preview");
              } catch (e) {
                setNotice(`打开预览窗出错：${e}`);
                return;
              }
              if (!overlayShown) await toggleOverlay();
            }}
          >
            ▶ 预览排练
          </button>
          <button className="btn ghost" onClick={toggleOverlay}>
            {overlayShown ? "隐藏字幕条" : "显示字幕条"}
          </button>
        </div>
      </header>
      {notice ? (
        <div className="notice" onClick={() => setNotice("")}>
          {notice}（点击关闭）
        </div>
      ) : null}

      <div className="tabs">
        <button className={tab === "script" ? "tab active" : "tab"} onClick={() => setTab("script")}>
          字幕稿
        </button>
        <button className={tab === "settings" ? "tab active" : "tab"} onClick={() => setTab("settings")}>
          外观与显示
        </button>
      </div>

      <main className={tab === "script" ? "content two-row" : "content"}>
        {tab === "script" ? (
          <>
                        <div className="preview-card">
              <div className="preview-label">
                当前字幕预览 · {settings.layout === "dock" ? "贴边布局" : "覆盖布局"}
                （与实际播放一致{settings.layout === "overlay" ? "，拖动字幕条可调位置" : ""}）
                {slideImg ? " · 幻灯片背景" : ""}
              </div>
              <div className="stage-wrap" ref={setWrapEl}>
              <div
                className="preview-stage"
                ref={stageRef}
                style={stageW > 0 ? { width: stageW, height: stageH } : undefined}
              >
                {/* 放映区：overlay=整块舞台；dock=让位后的剩余区（幻灯片贴在其中） */}
                <div
                  className="slide-area"
                  style={{
                    left: slideRect.left,
                    top: slideRect.top,
                    width: slideRect.width,
                    height: slideRect.height,
                  }}
                >
                  {slideImg ? (
                    <img className="slide-img" src={slideImg} alt={`第 ${slide} 页`} draggable={false} />
                  ) : (
                    <div className="slide-none">未导入预览图（导入 PPT 后显示）</div>
                  )}
                </div>
                {/* 字幕条窗口矩形：位置/尺寸 = 真实 overlay 窗口落位（bar_rect 同公式） */}
                <div
                  className={`bar-win ${settings.layout === "overlay" ? "draggable" : ""} ${dragging ? "dragging" : ""}`}
                  style={{
                    left: winRect.left,
                    top: winRect.top,
                    width: winRect.width,
                    height: winRect.height,
                  }}
                  onPointerDown={onBarPointerDown}
                  onPointerMove={onBarPointerMove}
                  onPointerUp={onBarPointerUp}
                >
                  <BarView
                    settings={settings}
                    cues={pageCueList}
                    mode={settings.advance}
                    stepIndex={Math.min(pbStatus?.index ?? 0, Math.max(0, pageCueList.length - 1))}
                    offset={0}
                    paused={pbStatus?.paused}
                    empty={pageCueList.length === 0}
                    scale={k}
                  />
                </div>
              </div>
              </div>
              <div className="preview-meta">
                <input
                  className="page-input"
                  type="number"
                  min={1}
                  value={slide}
                  onChange={(e) => goSlide(Number(e.target.value))}
                />
                页 · {settings.advance === "scroll" ? "滚动" : "逐条"} · 本页 {pageIdx.length} 条
                {settings.advance === "step" && pbStatus ? ` · 第 ${Math.min(pbStatus.index + 1, pbStatus.total)}/${pbStatus.total} 条` : ""}
                {pbStatus?.paused ? " · 已暂停" : ""}
                {pbStatus?.atEnd ? " · 已到页尾" : ""}
                {posCustom ? (
                  <button className="btn ghost mini" onClick={resetBarPos} title="回到底部居中默认位置">
                    恢复默认位置
                  </button>
                ) : null}
              </div>
            </div>

            <section className="editor">
              <textarea
                className="script-input"
                spellCheck={false}
                value={script}
                onChange={(e) => setScript(e.target.value)}
                placeholder={"=== 1 ===\n第一句字幕\n第二句字幕\n\n=== 2 ===\n下一页的字幕"}
              />
              <p className="hint">
                <code>=== 3 ===</code> 表示这一页 PPT；<code>=== 3-5 ===</code> 表示第 3 到 5 页共用；
                一行一条字幕，<code>EN:</code> 开头是英文行（可选）。
              </p>
            </section>

            <section className="side">
              <div className="controls">
                <button className="btn" onClick={() => pageStep(-1)}>
                  ← 上一页
                </button>
                <button className="btn" onClick={() => pageStep(1)}>
                  下一页 →
                </button>
                <button className="btn ghost" onClick={() => sendCtrl("pause")}>
                  {pbStatus?.paused ? "▶ 继续" : "⏸ 暂停"}
                </button>
                <button className="btn ghost" onClick={() => sendCtrl("restart")}>
                  ↺ 回本页开头
                </button>
              </div>
              <div className="controls">
                <button className="btn primary" onClick={() => sendCtrl("step", -1)} disabled={settings.advance === "scroll"}>
                  F8 上一条
                </button>
                <button className="btn primary" onClick={() => sendCtrl("step", 1)} disabled={settings.advance === "scroll"}>
                  F9 下一条
                </button>
              </div>

              <div className="page-list">
                {pages.map((p) => (
                  <button
                    key={p}
                    className={p === slide ? "page-chip active" : "page-chip"}
                    onClick={() => goSlide(p)}
                  >
                    P{p}
                  </button>
                ))}
              </div>
            </section>
          </>
        ) : (
          <section className="settings">
            <div className="field">
              <label>布局（R16）</label>
              <select
                value={settings.layout}
                onChange={(e) => setSettings({ ...settings, layout: e.target.value as Settings["layout"] })}
              >
                <option value="overlay">覆盖 · 字幕条浮在放映上</option>
                <option value="dock">贴边 · 放映窗让出一条，字幕不遮挡</option>
              </select>
            </div>

            {settings.layout === "dock" ? (
              <>
                <div className="field">
                  <label>字幕条贴哪一侧</label>
                  <select
                    value={settings.dockSide}
                    onChange={(e) => setSettings({ ...settings, dockSide: e.target.value as Settings["dockSide"] })}
                  >
                    <option value="top">上（默认，紧贴放映窗上沿）</option>
                    <option value="bottom">下（像电影字幕）</option>
                    <option value="left">左</option>
                    <option value="right">右</option>
                  </select>
                </div>
                <div className="field">
                  <label>
                    字幕条占比 <b>{Math.round(settings.dockRatio * 100)}%</b>
                  </label>
                  <input
                    type="range"
                    min={8}
                    max={50}
                    value={Math.round(settings.dockRatio * 100)}
                    onChange={(e) => setSettings({ ...settings, dockRatio: Number(e.target.value) / 100 })}
                  />
                </div>
              </>
            ) : null}

            <div className="field">
              <label>推进模式（运行中可切）</label>
              <select
                value={settings.advance}
                onChange={(e) => setSettings({ ...settings, advance: e.target.value as Settings["advance"] })}
              >
                <option value="scroll">整段滚动 · 进页自动上滚</option>
                <option value="step">逐条推进 · F9 下一条</option>
              </select>
            </div>

            <div className="field">
              <label>
                基础显示行数 <b>{settings.lines}</b>（折行超出自动增高，封顶 5）
              </label>
              <input
                type="range"
                min={1}
                max={5}
                value={settings.lines}
                onChange={(e) => setSettings({ ...settings, lines: Number(e.target.value) })}
              />
            </div>

            {settings.advance === "scroll" ? (
              <>
                <div className="field">
                  <label>
                    滚动速度 <b>{settings.scrollSpeed.toFixed(1)} 行/秒</b>
                  </label>
                  <input
                    type="range"
                    min={0.1}
                    max={2}
                    step={0.1}
                    value={settings.scrollSpeed}
                    onChange={(e) => setSettings({ ...settings, scrollSpeed: Number(e.target.value) })}
                  />
                </div>
                <div className="field">
                  <label>
                    进页停留 <b>{settings.scrollDelay} ms</b>
                  </label>
                  <input
                    type="range"
                    min={0}
                    max={3000}
                    step={100}
                    value={settings.scrollDelay}
                    onChange={(e) => setSettings({ ...settings, scrollDelay: Number(e.target.value) })}
                  />
                </div>
                <div className="field">
                  <label>
                    <input
                      type="checkbox"
                      checked={settings.stayAtEnd}
                      onChange={(e) => setSettings({ ...settings, stayAtEnd: e.target.checked })}
                    />
                    滚完停在最后一行（不淡出）
                  </label>
                </div>
              </>
            ) : null}

            <div className="field">
              <label>中文字号 <b>{settings.fontSize}</b></label>
              <input
                type="range"
                min={20}
                max={96}
                value={settings.fontSize}
                onChange={(e) => setSettings({ ...settings, fontSize: Number(e.target.value) })}
              />
            </div>

            <div className="field">
              <label>
                <input
                  type="checkbox"
                  checked={settings.showEn}
                  onChange={(e) => setSettings({ ...settings, showEn: e.target.checked })}
                />
                同时显示英文行
              </label>
            </div>

            <div className="field">
              <label>英文字号 <b>{settings.enSize}</b></label>
              <input
                type="range"
                min={12}
                max={48}
                value={settings.enSize}
                disabled={!settings.showEn}
                onChange={(e) => setSettings({ ...settings, enSize: Number(e.target.value) })}
              />
            </div>

            <div className="field">
              <label>字幕条宽度 <b>{Math.round(settings.widthRatio * 100)}%</b></label>
              <input
                type="range"
                min={40}
                max={100}
                value={Math.round(settings.widthRatio * 100)}
                onChange={(e) => setSettings({ ...settings, widthRatio: Number(e.target.value) / 100 })}
              />
            </div>

            <div className="field">
              <label>距屏幕底部 <b>{Math.round(settings.bottomRatio * 100)}%</b>{posCustom ? "（已自定义位置，以拖动位置为准）" : ""}</label>
              <input
                type="range"
                min={1}
                max={40}
                value={Math.round(settings.bottomRatio * 100)}
                disabled={posCustom}
                onChange={(e) => setSettings({ ...settings, bottomRatio: Number(e.target.value) / 100 })}
              />
            </div>

            <div className="field">
              <label>背景不透明度 <b>{Math.round((settings.bgAlpha / 255) * 100)}%</b></label>
              <input
                type="range"
                min={0}
                max={255}
                value={settings.bgAlpha}
                onChange={(e) => setSettings({ ...settings, bgAlpha: Number(e.target.value) })}
              />
            </div>

            <div className="field">
              <label>文字对齐</label>
              <select
                value={settings.align}
                onChange={(e) => setSettings({ ...settings, align: e.target.value as Settings["align"] })}
              >
                <option value="center">居中</option>
                <option value="left">左对齐</option>
              </select>
            </div>

            <div className="field">
              <label>字幕条所在显示器</label>
              <select
                value={settings.screen}
                onChange={(e) => setSettings({ ...settings, screen: e.target.value })}
              >
                <option value="auto">自动（跟随 PPT 放映的屏幕）</option>
                {monitors.map((m) => (
                  <option key={m.index} value={String(m.index)}>
                    {m.index}. {m.name} {m.width}×{m.height}
                    {m.primary ? "（主屏）" : ""}
                  </option>
                ))}
              </select>
            </div>

            <div className="field">
              <label>
                <input
                  type="checkbox"
                  checked={settings.textOnly}
                  onChange={(e) => setSettings({ ...settings, textOnly: e.target.checked })}
                />
                仅文字字幕（无背景板，更不遮挡画面）
              </label>
              <p className="field-hint">
                字幕条默认可直接拖动调位置；Ctrl+Alt+X 可临时切换点击穿透（放 transcript
                里一般不用开——穿透会把点击传给下层的放映画面）。
              </p>
            </div>

            <div className="field">
              <label>
                <input
                  type="checkbox"
                  checked={settings.alwaysOnTop}
                  onChange={(e) => setSettings({ ...settings, alwaysOnTop: e.target.checked })}
                />
                始终置顶（盖在 PPT 放映之上）
              </label>
            </div>

            <div className="hotkeys">
              <div className="hotkeys-title">快捷键</div>
              {HOTKEYS.map(([k, label]) => (
                <div className="hotkey-row" key={k}>
                  <kbd>{k}</kbd>
                  <span>{label}</span>
                </div>
              ))}
            </div>
          </section>
        )}
      </main>
    </div>
  );
}
