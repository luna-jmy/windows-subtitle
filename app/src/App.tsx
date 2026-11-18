import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Cue,
  DEFAULT_SETTINGS,
  Settings,
  cuesOfPage,
  fromRustSettings,
  parseScript,
  toRustSettings,
} from "./types";
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
  ["F9", "下一条字幕"],
  ["F8", "上一条字幕"],
  ["F7", "显示 / 隐藏字幕条"],
  ["Ctrl + Alt + P", "显示 / 隐藏这个控制台"],
  ["Ctrl + Alt + = / −", "放大 / 缩小字号"],
  ["Ctrl + Alt + Q", "退出程序"],
];

export default function App() {
  const [script, setScript] = useState("");
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [index, setIndex] = useState(0);
  const [slide, setSlide] = useState(1);
  const [overlayShown, setOverlayShown] = useState(false);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [saved, setSaved] = useState<"idle" | "saving" | "done">("idle");
  const [ready, setReady] = useState(false);
  const [tab, setTab] = useState<"script" | "settings">("script");
  const settingsRef = useRef(settings);
  const indexRef = useRef(0);

  settingsRef.current = settings;

  const cues: Cue[] = useMemo(() => parseScript(script), [script]);
  const current = cues[Math.min(index, Math.max(0, cues.length - 1))];
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
      }
    }, 600);
    return () => clearTimeout(t);
  }, [script, settings, ready]);

  // ---------- 推送字幕 / 样式 ----------
  useEffect(() => {
    if (!ready) return;
    indexRef.current = index;
    const cue = cues[Math.min(index, Math.max(0, cues.length - 1))];
    invoke("push_cue", {
      payload: {
        zh: cue?.zh ?? "",
        en: cue?.en ?? "",
        index,
        total: cues.length,
        page: cue?.page ?? 0,
        visible: cues.length > 0,
      },
    }).catch(console.error);
  }, [index, cues, ready]);

  useEffect(() => {
    if (!ready) return;
    invoke("push_style", { settings: toRustSettings(settings) }).catch(console.error);
  }, [settings, ready]);

  // ---------- 全局热键 ----------
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    const reg = async () => {
      unlisteners.push(
        await listen("hk:next", () => step(1)),
        await listen("hk:prev", () => step(-1)),
        await listen("hk:toggle", () => toggleOverlay()),
        await listen("hk:panel", () => invoke("toggle_console").catch(() => {})),
        await listen("hk:font-up", () => bumpFont(2)),
        await listen("hk:font-down", () => bumpFont(-2)),
        await listen("hk:quit", () => invoke("quit_app").catch(() => {}))
      );
    };
    reg();
    return () => unlisteners.forEach((u) => u());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cues]);

  function step(delta: number) {
    setIndex((i) => {
      const next = Math.max(0, Math.min(cues.length - 1, i + delta));
      const cue = cues[next];
      if (cue) setSlide(cue.page);
      return next;
    });
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
    }
  }

  async function goSlide(next: number) {
    const target = Math.max(1, next);
    setSlide(target);
    const list = cuesOfPage(cues, target);
    if (list.length) setIndex(list[0]);
  }

  const totalPages = cues.length ? Math.max(...cues.map((c) => c.pageEnd)) : 1;

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
          <span className="sep">·</span>
          {cues.length} 条 / {totalPages} 页
          <span className="sep">·</span>
          {saved === "saving" ? "保存中…" : "已保存"}
        </div>
        <div className="top-actions">
          <button className="btn ghost" onClick={toggleOverlay}>
            {overlayShown ? "隐藏字幕条" : "显示字幕条"}
          </button>
        </div>
      </header>

      <div className="tabs">
        <button className={tab === "script" ? "tab active" : "tab"} onClick={() => setTab("script")}>
          字幕稿
        </button>
        <button className={tab === "settings" ? "tab active" : "tab"} onClick={() => setTab("settings")}>
          外观与显示
        </button>
      </div>

      <main className="content">
        {tab === "script" ? (
          <>
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
              <div className="preview-card">
                <div className="preview-label">当前字幕预览</div>
                <div className="preview-bar" data-align={settings.align}>
                  <div className="preview-zh" style={{ fontSize: Math.min(34, settings.fontSize * 0.62) }}>
                    {current?.zh || "（这一条是空的）"}
                  </div>
                  {settings.showEn && current?.en ? (
                    <div className="preview-en" style={{ fontSize: Math.min(20, settings.enSize * 0.7) }}>
                      {current.en}
                    </div>
                  ) : null}
                </div>
                <div className="preview-meta">
                  <input
                    className="page-input"
                    type="number"
                    min={1}
                    value={slide}
                    onChange={(e) => goSlide(Number(e.target.value))}
                  />
                  页 · 第 {cues.length ? index + 1 : 0} / {cues.length} 条
                </div>
              </div>

              <div className="controls">
                <button className="btn" onClick={() => goSlide(slide - 1)}>
                  ← 上一页
                </button>
                <button className="btn" onClick={() => goSlide(slide + 1)}>
                  下一页 →
                </button>
              </div>
              <div className="controls">
                <button className="btn primary" onClick={() => step(-1)}>
                  F8 上一条
                </button>
                <button className="btn primary" onClick={() => step(1)}>
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
              <label>距屏幕底部 <b>{Math.round(settings.bottomRatio * 100)}%</b></label>
              <input
                type="range"
                min={1}
                max={40}
                value={Math.round(settings.bottomRatio * 100)}
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
                  checked={settings.clickThrough}
                  onChange={(e) => setSettings({ ...settings, clickThrough: e.target.checked })}
                />
                鼠标点击穿透（推荐开着，避免挡住 PPT 操作）
              </label>
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
