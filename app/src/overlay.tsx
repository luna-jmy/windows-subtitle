// 字幕条窗口：播放状态机自治（PRD 6.2/6.3 模式矩阵）
// 滚动模式：整段连续上滚，滚完停末行；逐条模式：F9/F8 逐条推进
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SETTINGS, fromRustSettings, Settings } from "./types";
import "./overlay.css";

type PageCue = { zh: string; en: string | null };
type PageEvent = { page: number; cues: PageCue[] };
type CtrlEvent = { kind: string; arg: number };
type Status = { page: number; index: number; total: number; paused: boolean; advance: string; atEnd: boolean };

function hexToRgba(hex: string, alpha255: number): string {
  const m = hex.replace("#", "");
  const n = parseInt(m.length === 3 ? m.split("").map((c) => c + c).join("") : m, 16);
  const a = Math.round((alpha255 / 255) * 100) / 100;
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

function Overlay() {
  const [page, setPage] = useState(1);
  const [cues, setCues] = useState<PageCue[]>([]);
  const [s, setS] = useState<Settings>(DEFAULT_SETTINGS);
  const [stepIndex, setStepIndex] = useState(0);
  const [paused, setPaused] = useState(false);
  const [atEnd, setAtEnd] = useState(false);
  const [offset, setOffset] = useState(0); // 滚动像素偏移

  const rollRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef({ y: 0, delayUntil: 0 });
  const advanceRef = useRef<"scroll" | "step">("scroll");

  const lineHeight = s.fontSize * 1.5;
  const isScroll = s.advance === "scroll";
  const stepBasis = Math.round(lineHeight * s.lines);

  // ---------- 事件接入 ----------
  useEffect(() => {
    // 通知主界面：overlay 的 JS 已就绪，可以 push_page 了
    emit("overlay-ready", null).catch(() => {});
    const un: Array<() => void> = [];
    (async () => {
      un.push(
        await listen<PageEvent>("page", (e) => {
          setPage(e.payload.page);
          setCues(e.payload.cues);
          setStepIndex(0);
          setPaused(false);
          scrollRef.current = { y: 0, delayUntil: 0 };
          setOffset(0);
          setAtEnd(false);
        }),
        await listen<any>("style", (e) => {
          const next = fromRustSettings(e.payload);
          if (next.advance !== advanceRef.current) {
            // 推进模式切换：重置播放状态（PRD 6.3 模式矩阵）
            advanceRef.current = next.advance;
            scrollRef.current = { y: 0, delayUntil: 0 };
            setOffset(0);
            setStepIndex(0);
            setAtEnd(false);
            setPaused(false);
          }
          setS(next);
        }),
        await listen<CtrlEvent>("ctrl", (e) => handleCtrl(e.payload.kind, e.payload.arg)),
        // 热键直达 overlay（app.emit 广播），按模式矩阵生效
        await listen("hk:next", () => stepCue(1)),
        await listen("hk:prev", () => stepCue(-1)),
        await listen("hk:pause", () => setPaused((p) => !p)),
        await listen("hk:nudge-up", () => nudge(-1)),
        await listen("hk:nudge-down", () => nudge(1)),
        await listen("hk:restart", () => restart()),
      );
    })();
    return () => un.forEach((f) => f());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ---------- 播放控制 ----------
  function stepCue(dir: number) {
    if (isScroll) return; // 滚动模式下 F9/F8 无效（模式矩阵）
    setStepIndex((i) => Math.max(0, Math.min(cues.length - 1, i + dir)));
  }

  function nudge(dir: number) {
    if (!isScroll) return;
    const max = Math.max(0, (rollRef.current?.scrollHeight ?? 0) - (viewRef.current?.clientHeight ?? 0));
    scrollRef.current.y = Math.max(0, Math.min(max, scrollRef.current.y + dir * lineHeight));
    setOffset(scrollRef.current.y);
    setAtEnd(scrollRef.current.y >= max - 1);
  }

  function restart() {
    scrollRef.current.y = 0;
    scrollRef.current.delayUntil = performance.now() + s.scrollDelay;
    setOffset(0);
    setAtEnd(false);
    setPaused(false);
    if (!isScroll) setStepIndex(0);
  }

  function handleCtrl(kind: string, arg: number) {
    if (kind === "step") stepCue(arg);
    else if (kind === "restart") restart();
    else if (kind === "pause") setPaused((p) => !p);
  }

  // ---------- 滚动引擎（rAF） ----------
  useEffect(() => {
    if (!isScroll || cues.length === 0) return;
    const pxPerSec = s.scrollSpeed * lineHeight;
    let raf = 0;
    let last = performance.now();
    if (scrollRef.current.delayUntil === 0) {
      scrollRef.current.delayUntil = last + s.scrollDelay; // 进页先停一拍（scrollDelay）
    }
    const tick = (now: number) => {
      const dt = (now - last) / 1000;
      last = now;
      const st = scrollRef.current;
      if (!paused && now >= st.delayUntil) {
        st.y += pxPerSec * dt;
        const max = Math.max(0, (rollRef.current?.scrollHeight ?? 0) - (viewRef.current?.clientHeight ?? 0));
        if (st.y >= max) {
          if (s.stayAtEnd) {
            st.y = max; // 滚完停末行，不淡出不滚飞
            setAtEnd(true);
          }
          // stayAtEnd=false：不钳制，内容继续滚出
        }
        setOffset(st.y);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [isScroll, cues, s.scrollSpeed, s.scrollDelay, s.fontSize, s.stayAtEnd, paused, lineHeight]);

  // ---------- 状态回报给主界面 ----------
  useEffect(() => {
    const st: Status = {
      page,
      index: isScroll ? 0 : stepIndex,
      total: cues.length,
      paused,
      advance: s.advance,
      atEnd,
    };
    emit("playback-status", st).catch(() => {});
  }, [page, stepIndex, cues.length, paused, s.advance, atEnd]);

  // ---------- 内容折行超行时自报窗口高度（R18：基础行数起步、按实际长、封顶 5 行） ----------
  useLayoutEffect(() => {
    const el = viewRef.current;
    if (!el) return;
    const pad2 = s.padding * 2 + (s.showEn ? Math.round(s.enSize * 1.5) : 0);
    const used = Math.round(el.scrollHeight);
    const basis = stepBasis;
    const cap = Math.round(lineHeight * 5);
    const want = Math.round(Math.max(basis, Math.min(used, cap)) + pad2);
    invoke("set_overlay_height", { height: want }).catch(() => {});
  }, [cues, stepIndex, s.fontSize, s.lines, s.padding, s.showEn, s.enSize, stepBasis, lineHeight]);

  const empty = cues.length === 0; // 无字幕页：底板保留、文字清空（PRD 边界行为）
  const current = !isScroll ? cues[Math.min(stepIndex, cues.length - 1)] : null;

  return (
    <div className="overlay-root" data-advance={s.advance}>
      <div
        className={`bar ${empty ? "empty" : ""} ${paused && !empty ? "paused" : ""}`}
        data-align={s.align}
        data-tauri-drag-region
        style={{
          background: hexToRgba(s.bgColor, s.bgAlpha),
          borderRadius: s.radius,
          padding: s.padding,
          fontFamily: `"${s.fontFamily}", sans-serif`,
          color: s.fg,
        }}
      >
        {empty ? (
          <div className="viewport" ref={viewRef} style={{ height: stepBasis }} />
        ) : isScroll ? (
          <div className="viewport" ref={viewRef} style={{ height: stepBasis }}>
            <div
              className="roll"
              ref={rollRef}
              style={{ transform: `translateY(${-Math.round(offset)}px)`, fontSize: s.fontSize, lineHeight: 1.5 }}
            >
              {cues.map((c, i) => (
                <p key={i} className="cue">
                  {c.zh}
                  {s.showEn && c.en ? (
                    <span className="en" style={{ fontSize: s.enSize }}>{c.en}</span>
                  ) : null}
                </p>
              ))}
            </div>
          </div>
        ) : (
          <div className="viewport" ref={viewRef}>
            <div className="roll" style={{ fontSize: s.fontSize, lineHeight: 1.5 }}>
              <p className="cue">
                {current?.zh}
                {s.showEn && current?.en ? (
                  <span className="en" style={{ fontSize: s.enSize }}>{current.en}</span>
                ) : null}
              </p>
            </div>
          </div>
        )}
      </div>
      {atEnd && isScroll && !empty ? <div className="endmark" title="已滚到本页末尾" /> : null}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Overlay />);
