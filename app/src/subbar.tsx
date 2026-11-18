// 字幕条共享渲染器（硬伤①②）：真实 overlay 窗口与主界面预览用同一份标记/样式，
// 所有外观设置（字号/颜色/圆角/内边距/中英文/对齐/行数）在两处表现完全一致。
// scale = 预览舞台宽 / 参考屏宽(1920)；真实 overlay 传 1（真实像素）。
import { Ref } from "react";
import { Settings } from "./types";
import "./overlay.css";

export type BarCue = { zh: string; en: string | null };

export type BarViewProps = {
  settings: Settings;
  cues: BarCue[];
  mode: "scroll" | "step";
  stepIndex: number;
  offset: number; // 滚动像素偏移（真实 overlay 的滚动引擎；预览固定 0 = 页首）
  paused?: boolean;
  empty?: boolean;
  scale: number;
  dragRegion?: boolean; // 真实 overlay 关穿透后可拖动；预览不传（避免把主窗口拖走）
  viewRef?: Ref<HTMLDivElement>;
  rollRef?: Ref<HTMLDivElement>;
};

export function hexToRgba(hex: string, alpha255: number): string {
  const m = hex.replace("#", "");
  const n = parseInt(m.length === 3 ? m.split("").map((c) => c + c).join("") : m, 16);
  const a = Math.round((alpha255 / 255) * 100) / 100;
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

export function barContentHeight(s: Settings): number {
  const lines = Math.min(5, Math.max(1, s.lines));
  // 与 lib.rs overlay_height 对应：内边距×2 + 行数×行高 + 英文行额外高
  const base = s.padding * 2 + s.fontSize * 1.5 * lines;
  const extra = s.showEn ? s.enSize * 1.6 : 0;
  return Math.round(base + extra);
}

export function BarView(props: BarViewProps) {
  const { settings: s, cues, mode, stepIndex, offset, paused, empty, scale, dragRegion } = props;
  const stepBasis = Math.round(s.fontSize * 1.5 * s.lines);
  const current = mode === "step" ? cues[Math.min(stepIndex, Math.max(0, cues.length - 1))] : null;

  return (
    <div
      className={`bar ${empty ? "empty" : ""} ${paused && !empty ? "paused" : ""}`}
      data-align={s.align}
      data-plate={s.textOnly ? "none" : "solid"}
      {...(dragRegion ? { "data-tauri-drag-region": true } : {})}
      style={{
        background: s.textOnly ? "transparent" : hexToRgba(s.bgColor, s.bgAlpha),
        borderRadius: s.textOnly ? 0 : s.radius * scale,
        padding: s.textOnly ? 0 : s.padding * scale,
        fontFamily: `"${s.fontFamily}", sans-serif`,
        color: s.fg,
      }}
    >
      {empty ? (
        <div
          className="viewport"
          ref={props.viewRef}
          style={{ height: stepBasis * scale }}
        />
      ) : mode === "scroll" ? (
        <div className="viewport" ref={props.viewRef} style={{ height: stepBasis * scale }}>
          <div
            className="roll"
            ref={props.rollRef}
            style={{
              transform: `translateY(${-Math.round(offset * scale)}px)`,
              fontSize: s.fontSize * scale,
              lineHeight: 1.5,
            }}
          >
            {cues.map((c, i) => (
              <p key={i} className="cue">
                {c.zh}
                {s.showEn && c.en ? (
                  <span className="en" style={{ fontSize: s.enSize * scale }}>{c.en}</span>
                ) : null}
              </p>
            ))}
          </div>
        </div>
      ) : (
        <div className="viewport" ref={props.viewRef}>
          <div className="roll" style={{ fontSize: s.fontSize * scale, lineHeight: 1.5 }}>
            <p className="cue">
              {current?.zh}
              {s.showEn && current?.en ? (
                <span className="en" style={{ fontSize: s.enSize * scale }}>{current.en}</span>
              ) : null}
            </p>
          </div>
        </div>
      )}
    </div>
  );
}
