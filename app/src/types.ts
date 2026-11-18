export type Settings = {
  fontSize: number;
  enSize: number;
  bottomRatio: number;
  widthRatio: number;
  bgAlpha: number;
  showEn: boolean;
  clickThrough: boolean;
  alwaysOnTop: boolean;
  screen: string;
  followPpt: boolean;
  align: "center" | "left";
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

export type CuePayload = {
  zh: string;
  en: string;
  index: number;
  total: number;
  page: number;
  visible: boolean;
};

export const DEFAULT_SETTINGS: Settings = {
  fontSize: 40,
  enSize: 24,
  bottomRatio: 0.08,
  widthRatio: 0.86,
  bgAlpha: 232,
  showEn: false,
  clickThrough: true,
  alwaysOnTop: true,
  screen: "auto",
  followPpt: true,
  align: "center",
};

/** 把 Rust 的 snake_case 设置转成前端 camelCase */
export function fromRustSettings(raw: any): Settings {
  return {
    fontSize: raw.font_size ?? DEFAULT_SETTINGS.fontSize,
    enSize: raw.en_size ?? DEFAULT_SETTINGS.enSize,
    bottomRatio: raw.bottom_ratio ?? DEFAULT_SETTINGS.bottomRatio,
    widthRatio: raw.width_ratio ?? DEFAULT_SETTINGS.widthRatio,
    bgAlpha: raw.bg_alpha ?? DEFAULT_SETTINGS.bgAlpha,
    showEn: raw.show_en ?? DEFAULT_SETTINGS.showEn,
    clickThrough: raw.click_through ?? DEFAULT_SETTINGS.clickThrough,
    alwaysOnTop: raw.always_on_top ?? DEFAULT_SETTINGS.alwaysOnTop,
    screen: raw.screen ?? DEFAULT_SETTINGS.screen,
    followPpt: raw.follow_ppt ?? DEFAULT_SETTINGS.followPpt,
    align: (raw.align as Settings["align"]) ?? DEFAULT_SETTINGS.align,
  };
}

export function toRustSettings(s: Settings) {
  return {
    font_size: s.fontSize,
    en_size: s.enSize,
    bottom_ratio: s.bottomRatio,
    width_ratio: s.widthRatio,
    bg_alpha: s.bgAlpha,
    show_en: s.showEn,
    click_through: s.clickThrough,
    always_on_top: s.alwaysOnTop,
    screen: s.screen,
    follow_ppt: s.followPpt,
    align: s.align,
  };
}

const PAGE_RE = /^===\s*(\d+)\s*(?:-\s*(\d+))?\s*===?\s*$/;

/** 解析字幕稿：`=== 3 ===` 或 `=== 3-5 ===` 分页，一行一条，EN: 前缀为英文行 */
export function parseScript(text: string): Cue[] {
  const cues: Cue[] = [];
  let page = 0;
  let pageEnd = 0;
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
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
