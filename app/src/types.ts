// 设置模型：与 Rust Settings（lib.rs）逐字段对应，默认值 = PRD 5.0
export type Settings = {
  fontSize: number;
  enSize: number;
  fontFamily: string;
  fg: string;
  bgColor: string;
  bgAlpha: number;
  radius: number;
  padding: number;
  bottomRatio: number;
  widthRatio: number;
  lines: number; // 基础行数 1–5
  advance: "scroll" | "step"; // 整段滚动 / 逐条
  scrollSpeed: number; // 行/秒
  scrollDelay: number; // ms
  stayAtEnd: boolean;
  layout: "overlay" | "dock";
  dockSide: "top" | "bottom" | "left" | "right";
  dockRatio: number;
  pptResize: boolean;
  showEn: boolean;
  clickThrough: boolean;
  alwaysOnTop: boolean;
  screen: string;
  followPpt: boolean;
  minimizeToTray: boolean;
  align: "center" | "left"; // UI 额外项
};

export type Store = {
  settings: Settings;
  script: string;
};

export type Cue = {
  zh: string;
  en: string;
  page: number;
  pageEnd: number;
};

export const DEFAULT_SETTINGS: Settings = {
  fontSize: 36,
  enSize: 24,
  fontFamily: "Microsoft YaHei",
  fg: "#FFFFFF",
  bgColor: "#111318",
  bgAlpha: 235,
  radius: 14,
  padding: 18,
  bottomRatio: 0.055,
  widthRatio: 0.86,
  lines: 1,
  advance: "scroll",
  scrollSpeed: 0.5,
  scrollDelay: 1000,
  stayAtEnd: true,
  layout: "overlay",
  dockSide: "top",
  dockRatio: 0.18,
  pptResize: true,
  showEn: false,
  clickThrough: true,
  alwaysOnTop: true,
  screen: "auto",
  followPpt: true,
  minimizeToTray: true,
  align: "center",
};

/** Rust snake_case → 前端 camelCase（缺字段用默认值兜底，兼容旧 store.json） */
export function fromRustSettings(raw: any): Settings {
  const d = DEFAULT_SETTINGS;
  return {
    fontSize: raw?.font_size ?? d.fontSize,
    enSize: raw?.en_size ?? d.enSize,
    fontFamily: raw?.font_family ?? d.fontFamily,
    fg: raw?.fg ?? d.fg,
    bgColor: raw?.bg_color ?? d.bgColor,
    bgAlpha: raw?.bg_alpha ?? d.bgAlpha,
    radius: raw?.radius ?? d.radius,
    padding: raw?.padding ?? d.padding,
    bottomRatio: raw?.bottom_ratio ?? d.bottomRatio,
    widthRatio: raw?.width_ratio ?? d.widthRatio,
    lines: raw?.lines ?? d.lines,
    advance: (raw?.advance as Settings["advance"]) ?? d.advance,
    scrollSpeed: raw?.scroll_speed ?? d.scrollSpeed,
    scrollDelay: raw?.scroll_delay ?? d.scrollDelay,
    stayAtEnd: raw?.stay_at_end ?? d.stayAtEnd,
    layout: (raw?.layout as Settings["layout"]) ?? d.layout,
    dockSide: (raw?.dock_side as Settings["dockSide"]) ?? d.dockSide,
    dockRatio: raw?.dock_ratio ?? d.dockRatio,
    pptResize: raw?.ppt_resize ?? d.pptResize,
    showEn: raw?.show_en ?? d.showEn,
    clickThrough: raw?.click_through ?? d.clickThrough,
    alwaysOnTop: raw?.always_on_top ?? d.alwaysOnTop,
    screen: raw?.screen ?? d.screen,
    followPpt: raw?.follow_ppt ?? d.followPpt,
    minimizeToTray: raw?.minimize_to_tray ?? d.minimizeToTray,
    align: (raw?.align as Settings["align"]) ?? d.align,
  };
}

export function toRustSettings(s: Settings) {
  return {
    font_size: s.fontSize,
    en_size: s.enSize,
    font_family: s.fontFamily,
    fg: s.fg,
    bg_color: s.bgColor,
    bg_alpha: s.bgAlpha,
    radius: s.radius,
    padding: s.padding,
    bottom_ratio: s.bottomRatio,
    width_ratio: s.widthRatio,
    lines: s.lines,
    advance: s.advance,
    scroll_speed: s.scrollSpeed,
    scroll_delay: s.scrollDelay,
    stay_at_end: s.stayAtEnd,
    layout: s.layout,
    dock_side: s.dockSide,
    dock_ratio: s.dockRatio,
    ppt_resize: s.pptResize,
    show_en: s.showEn,
    click_through: s.clickThrough,
    always_on_top: s.alwaysOnTop,
    screen: s.screen,
    follow_ppt: s.followPpt,
    minimize_to_tray: s.minimizeToTray,
    align: s.align,
  };
}

const PAGE_RE = /^===\s*(\d+)\s*(?:-\s*(\d+))?\s*===?\s*$/;

/** 编辑期快速解析（与 Rust script.rs 语义一致；校验/报错走 validate_script 命令） */
export function parseScript(text: string): Cue[] {
  const cues: Cue[] = [];
  let page = 0;
  let pageEnd = 0;
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    const m = line.match(PAGE_RE);
    if (m) {
      page = Number(m[1]);
      pageEnd = m[2] ? Number(m[2]) : page;
      continue;
    }
    if (/^EN\s*[:：]/i.test(line)) {
      const en = line.replace(/^EN\s*[:：]\s*/i, "");
      const last = cues[cues.length - 1];
      if (last && !last.en) {
        last.en = en;
        continue;
      }
      cues.push({ zh: "", en, page: page || Math.max(1, pageEnd), pageEnd: pageEnd || page });
      continue;
    }
    cues.push({ zh: line, en: "", page: page || 1, pageEnd: pageEnd || page || 1 });
  }
  return cues;
}

export function cuesOfPage(cues: Cue[], slide: number): number[] {
  const idx: number[] = [];
  cues.forEach((c, i) => {
    if (slide >= c.page && slide <= c.pageEnd) idx.push(i);
  });
  return idx;
}
