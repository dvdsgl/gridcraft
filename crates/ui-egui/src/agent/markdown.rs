//! A small Markdown subset for the agent's replies: paragraphs, headings, bullet and numbered
//! lists, fenced code, `inline code`, **bold** and *italic*. Anything else shows as written.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId};

/// Colours and sizes a reply is drawn with.
pub(crate) struct Style {
    pub text: Color32,
    pub dim: Color32,
    pub code: Color32,
    pub size: f32,
    pub line_height: f32,
}

fn font(bold: bool, size: f32) -> FontId {
    if bold { crate::theme::ui_bold(size) } else { crate::theme::ui_font(size) }
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::theme::MONO.into()))
}

/// Lays `md` out as one job, wrapped at `width`.
pub(crate) fn layout(md: &str, width: f32, st: &Style) -> LayoutJob {
    let mut job = LayoutJob { wrap: egui::text::TextWrapping { max_width: width, ..Default::default() }, ..Default::default() };
    let base = TextFormat { font_id: font(false, st.size), color: st.text, line_height: Some(st.line_height), ..Default::default() };
    let gap = TextFormat { font_id: font(false, st.size * 0.5), line_height: Some(st.line_height * 0.45), ..base.clone() };
    let mut in_code = false;
    let mut first = true;
    let mut blank = false;
    for raw in md.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if !first {
            job.append("\n", 0.0, base.clone());
        }
        if line.trim().is_empty() && !in_code {
            // A blank line separates paragraphs: a short gap rather than a full empty line.
            if !blank && !first {
                job.append(" ", 0.0, gap.clone());
            }
            blank = true;
            continue;
        }
        first = false;
        blank = false;
        if in_code {
            job.append(line, 0.0, TextFormat { font_id: mono(st.size - 1.5), color: st.dim, ..base.clone() });
            continue;
        }
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()).min(8) as f32 * 3.0;
        if let Some(h) = heading(trimmed) {
            inline(&mut job, h, &TextFormat { font_id: font(true, st.size + 1.0), ..base.clone() }, st);
        } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            job.append("•  ", indent, TextFormat { color: st.dim, ..base.clone() });
            inline(&mut job, rest, &base, st);
        } else if let Some((n, rest)) = numbered(trimmed) {
            job.append(&format!("{n}.  "), indent, TextFormat { color: st.dim, ..base.clone() });
            inline(&mut job, rest, &base, st);
        } else if trimmed.starts_with('|') {
            // Tables keep their columns in a fixed-width face.
            job.append(trimmed, indent, TextFormat { font_id: mono(st.size - 1.5), ..base.clone() });
        } else {
            inline(&mut job, trimmed, &base, st);
        }
    }
    job
}

fn heading(s: &str) -> Option<&str> {
    let hashes = s.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes).then(|| s.get(hashes..)).flatten().and_then(|r| r.strip_prefix(' '))
}

fn numbered(s: &str) -> Option<(&str, &str)> {
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let (n, rest) = s.split_at(digits);
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")).map(|r| (n, r))
}

/// `**bold**`, `*italic*` and `` `code` `` within one line.
fn inline(job: &mut LayoutJob, s: &str, base: &TextFormat, st: &Style) {
    let mut bold = false;
    let mut italic = false;
    let mut buf = String::new();
    let flush = |job: &mut LayoutJob, buf: &mut String, bold: bool, italic: bool| {
        if buf.is_empty() {
            return;
        }
        let mut f = base.clone();
        if bold {
            f.font_id = font(true, base.font_id.size);
        }
        f.italics = italic;
        job.append(buf, 0.0, f);
        buf.clear();
    };
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars.get(i).copied().unwrap_or(' ');
        let next = chars.get(i + 1).copied();
        if c == '`' {
            // Inline code runs to the next backtick; an unmatched one is kept as text.
            if let Some(end) = chars.iter().skip(i + 1).position(|x| *x == '`') {
                flush(job, &mut buf, bold, italic);
                let code: String = chars.iter().skip(i + 1).take(end).collect();
                job.append(&code, 0.0, TextFormat { font_id: mono(base.font_id.size - 1.5), color: st.code, ..base.clone() });
                i += end + 2;
                continue;
            }
        } else if c == '*' && next == Some('*') {
            flush(job, &mut buf, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        } else if c == '*' && (italic || next.is_some_and(|n| !n.is_whitespace())) {
            flush(job, &mut buf, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    flush(job, &mut buf, bold, italic);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> Style {
        Style { text: Color32::BLACK, dim: Color32::GRAY, code: Color32::BLUE, size: 13.0, line_height: 20.0 }
    }

    fn text(job: &LayoutJob) -> String {
        job.text.clone()
    }

    #[test]
    fn emphasis_and_code_lose_their_markers() {
        let job = layout("Q3 fell, mostly from **Cold brew** and `=TREND(C11:F11)` *now*.", 300.0, &st());
        assert_eq!(text(&job), "Q3 fell, mostly from Cold brew and =TREND(C11:F11) now.");
        assert!(job.sections.iter().any(|s| s.format.color == Color32::BLUE), "code is coloured");
    }

    #[test]
    fn lists_headings_and_fences() {
        let job = layout("## Plan\n1. Add a column\n- Forecast\n\n```\nx = 1\n```", 300.0, &st());
        assert_eq!(text(&job), "Plan\n1.  Add a column\n•  Forecast\n \nx = 1");
    }

    #[test]
    fn unmatched_markers_stay() {
        assert_eq!(text(&layout("a * b and `c", 300.0, &st())), "a * b and `c");
    }
}
