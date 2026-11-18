// 字幕稿解析/序列化（PRD 5.1 格式）
// 一行 = 一条字幕；`=== N ===` / `=== N-M ===` 分页；`#` 注释；`EN:` 附着到上一条中文

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptCue {
    pub zh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
    pub page_start: u32,
    pub page_end: u32,
}

#[derive(Debug, Serialize)]
pub struct ScriptSummary {
    pub cues: usize,
    pub pages: usize,
    pub first_page: u32,
    pub last_page: u32,
}

#[derive(Debug)]
pub struct ScriptError {
    pub line_no: usize,
    pub message: String,
}

/// 识别 `=== N ===` / `=== N-M ===`；像页标记但写错的行直接报错
fn page_marker(line: &str) -> Result<Option<(u32, u32)>, String> {
    let t = line.trim();
    if !t.starts_with("===") || !t.ends_with('=') {
        return Ok(None); // 普通内容行
    }
    let inner = t.trim_start_matches('=').trim_end_matches('=').trim();
    if inner.is_empty() {
        return Err(format!("页标记「{t}」里没有页码"));
    }
    let (a, b) = match inner.split_once('-') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (inner, inner),
    };
    let parse_num = |s: &str| {
        s.parse::<u32>()
            .map_err(|_| format!("页标记「{t}」里的页码「{s}」不是数字"))
    };
    let start = parse_num(a)?;
    let end = parse_num(b)?;
    if start == 0 || end == 0 {
        return Err(format!("页码必须从 1 开始（「{t}」）"));
    }
    if start > end {
        return Err(format!("页区间起点大于终点（「{t}」）"));
    }
    Ok(Some((start, end)))
}

/// 识别 `EN: xxx` / `EN：xxx`（大小写不敏感），返回英文内容
fn split_en(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    if b.len() < 3 || (b[0] | 0x20) != b'e' || (b[1] | 0x20) != b'n' {
        return None;
    }
    if b[2] == b':' {
        Some(line[3..].trim_start())
    } else if line[2..].starts_with('：') {
        Some(line[5..].trim_start())
    } else {
        None
    }
}

pub fn parse_script(text: &str) -> Result<Vec<ScriptCue>, ScriptError> {
    let mut cues: Vec<ScriptCue> = Vec::new();
    let mut cur: Option<(u32, u32)> = None;
    let mut cue_in_page = false; // 当前页标记后是否已有中文条（EN 必须跟在其后）
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let no = i + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match page_marker(line) {
            Err(m) => return Err(ScriptError { line_no: no, message: m }),
            Ok(Some((s, e))) => {
                cur = Some((s, e));
                cue_in_page = false;
                continue;
            }
            Ok(None) => {}
        }
        if let Some(en) = split_en(line) {
            if en.is_empty() {
                return Err(ScriptError { line_no: no, message: "EN 行内容为空".into() });
            }
            if !cue_in_page {
                return Err(ScriptError {
                    line_no: no,
                    message: "EN 行必须跟在同页的中文字幕后".into(),
                });
            }
            let last = cues.last_mut().expect("cue_in_page 蕴含已有条目");
            if last.en.is_some() {
                return Err(ScriptError { line_no: no, message: "一条字幕只能有一行 EN".into() });
            }
            last.en = Some(en.to_string());
            continue;
        }
        let (s, e) = cur.ok_or_else(|| ScriptError {
            line_no: no,
            message: "内容出现在第一个 === 页标记之前".into(),
        })?;
        cues.push(ScriptCue { zh: line.to_string(), en: None, page_start: s, page_end: e });
        cue_in_page = true;
    }
    Ok(cues)
}

/// 序列化：连续同页的条目归到一个页标记下；与 parse_script 互为往返
/// （导出/保存工程稿时用；暂无调用方，保留工具函数）
#[allow(dead_code)]
pub fn serialize_script(cues: &[ScriptCue]) -> String {
    let mut out = String::from("# 桌面字幕器字幕稿（一行=一条；=== N === 分页；EN: 附英文）\n");
    let mut cur: Option<(u32, u32)> = None;
    for c in cues {
        let key = (c.page_start, c.page_end);
        if cur != Some(key) {
            if cur.is_some() {
                out.push('\n');
            }
            if c.page_start == c.page_end {
                out.push_str(&format!("=== {} ===\n", c.page_start));
            } else {
                out.push_str(&format!("=== {}-{} ===\n", c.page_start, c.page_end));
            }
            cur = Some(key);
        }
        out.push_str(&c.zh);
        out.push('\n');
        if let Some(en) = &c.en {
            out.push_str("EN: ");
            out.push_str(en);
            out.push('\n');
        }
    }
    out
}

pub fn summarize(cues: &[ScriptCue]) -> ScriptSummary {
    use std::collections::BTreeSet;
    let mut pages = BTreeSet::new();
    let mut first = u32::MAX;
    let mut last = 0;
    for c in cues {
        for p in c.page_start..=c.page_end {
            pages.insert(p);
        }
        first = first.min(c.page_start);
        last = last.max(c.page_end);
    }
    ScriptSummary {
        cues: cues.len(),
        pages: pages.len(),
        first_page: if cues.is_empty() { 0 } else { first },
        last_page: last,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# 员工大会 · Amir\n\n=== 1 ===\n欢迎大家。\n今天想聊三件事。\n\n=== 2 ===\n先用二十分钟，讲一下今年我们走到了哪一步。\nEN: Twenty minutes on where we are.\n\n=== 3-4 ===\n（一页讲两屏时用区间）\n";

    fn cue(zh: &str, en: Option<&str>, s: u32, e: u32) -> ScriptCue {
        ScriptCue { zh: zh.into(), en: en.map(|x| x.into()), page_start: s, page_end: e }
    }

    #[test]
    fn parse_sample() {
        let cues = parse_script(SAMPLE).unwrap();
        assert_eq!(
            cues,
            vec![
                cue("欢迎大家。", None, 1, 1),
                cue("今天想聊三件事。", None, 1, 1),
                cue("先用二十分钟，讲一下今年我们走到了哪一步。", Some("Twenty minutes on where we are."), 2, 2),
                cue("（一页讲两屏时用区间）", None, 3, 4),
            ]
        );
        let s = summarize(&cues);
        assert_eq!((s.cues, s.pages, s.first_page, s.last_page), (4, 4, 1, 4));
    }

    #[test]
    fn roundtrip() {
        let once = parse_script(SAMPLE).unwrap();
        let text = serialize_script(&once);
        let twice = parse_script(&text).unwrap();
        assert_eq!(once, twice);
        // 再序列化一次应稳定（幂等）
        assert_eq!(serialize_script(&twice), text);
    }

    #[test]
    fn roundtrip_reshuffled_pages() {
        // 同一页码不连续出现时，序列化会分块写页头，往返仍应保持条目顺序
        let cues = vec![cue("a", None, 1, 1), cue("b", None, 2, 2), cue("c", None, 1, 1)];
        let twice = parse_script(&serialize_script(&cues)).unwrap();
        assert_eq!(cues, twice);
    }

    #[test]
    fn err_zero_page() {
        let e = parse_script("=== 0 ===\n内容").unwrap_err();
        assert_eq!(e.line_no, 1);
        assert!(e.message.contains("从 1 开始"));
    }

    #[test]
    fn err_range_order() {
        let e = parse_script("=== 5-3 ===\n内容").unwrap_err();
        assert!(e.message.contains("起点大于终点"));
    }

    #[test]
    fn err_content_before_marker() {
        let e = parse_script("直接开讲\n=== 1 ===\n内容").unwrap_err();
        assert!(e.message.contains("第一个 === 页标记之前"));
    }

    #[test]
    fn err_malformed_marker() {
        let e = parse_script("=== x9 ===\n内容").unwrap_err();
        assert!(e.message.contains("不是数字"));
    }

    #[test]
    fn err_en_rules() {
        // EN 出现在页头（前面没有同页中文条）
        assert!(parse_script("=== 1 ===\nEN: orphan").unwrap_err().message.contains("同页"));
        // 一条字幕两行 EN
        assert!(parse_script("=== 1 ===\n中文\nEN: a\nEN: b").unwrap_err().message.contains("只能有一行"));
    }

    #[test]
    fn comments_and_blank_skipped() {
        let cues = parse_script("# 标题\n\n=== 1 ===\n# 备注\n\n中文一\n\n\n中文二\n").unwrap();
        assert_eq!(cues, vec![cue("中文一", None, 1, 1), cue("中文二", None, 1, 1)]);
    }
}
