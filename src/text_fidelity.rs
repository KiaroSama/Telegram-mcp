//! Telegram text projection, UTF-16 entity rebasing, and grapheme-safe bounding.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use unicode_normalization::char::canonical_combining_class;

pub const DEFAULT_MAX_TEXT_CHARS: usize = 4096;
pub const DEFAULT_MAX_NAME_CHARS: usize = 256;
pub const EMPTY_PLACEHOLDER: &str = "[empty]";
pub const TRUNCATED_MARKER: &str = "... [truncated]";
pub const ELLIPSIS_MARKER: &str = "…";
pub const RGI_TAG_SPECS: &[&str] = &["gbeng", "gbsct", "gbwls"];

const TAG_BASE: char = '\u{1F3F4}';
const TAG_END: char = '\u{E007F}';
const ZWNJ: char = '\u{200C}';
const ZWJ: char = '\u{200D}';
const SINGLE_SEPS: &str = "\r\n\t\x0B\x0C\u{85}\u{2028}\u{2029}";
const BREAK_SEPS: &str = "\r\x0B\x0C\u{85}";

#[derive(Debug, Clone, PartialEq)]
pub struct RawTextEntity {
    pub kind: String,
    pub offset: i64,
    pub length: i64,
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextEntity {
    #[serde(rename = "type")]
    pub kind: String,
    pub offset: usize,
    pub length: usize,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextFidelityMetadata {
    pub modified: bool,
    pub truncated: bool,
    pub empty: bool,
    pub original_utf16_len: usize,
    pub content_utf16_len: usize,
    pub output_utf16_len: usize,
    pub clipped_entities: usize,
    pub dropped_entities: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextFidelityResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<TextEntity>,
    pub metadata: TextFidelityMetadata,
    #[serde(skip)]
    pub boundary_map: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProjectionOptions {
    pub normalize_breaks: bool,
    pub collapse_newlines: bool,
    pub single_line: bool,
    pub trim: bool,
    pub empty_placeholder: bool,
    pub strip_ascii_joiners: bool,
    pub max_chars: Option<usize>,
    pub truncation_marker: &'static str,
}

impl ProjectionOptions {
    pub fn fidelity() -> Self {
        Self::default()
    }
    pub fn sanitized(max: usize) -> Self {
        Self {
            normalize_breaks: true,
            collapse_newlines: true,
            trim: true,
            empty_placeholder: true,
            strip_ascii_joiners: true,
            max_chars: Some(max),
            truncation_marker: TRUNCATED_MARKER,
            ..Self::default()
        }
    }
    pub fn display_text(max: usize) -> Self {
        Self {
            normalize_breaks: true,
            max_chars: Some(max),
            ..Self::default()
        }
    }
    pub fn display_name(max: usize) -> Self {
        Self {
            single_line: true,
            trim: true,
            max_chars: Some(max),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy)]
struct Tracked(char, usize, usize, bool);

fn re_hit(idx: usize, ch: char) -> bool {
    static RE: OnceLock<[Option<Regex>; 5]> = OnceLock::new();
    let set = RE.get_or_init(|| {
        [
            Regex::new(r"^[\p{Cc}\p{Cf}\u{180E}\u{2028}\u{2029}\u{E0000}-\u{E007F}]$").ok(),
            Regex::new(r"^[\p{Grapheme_Extend}\u{200C}\u{1F3FB}-\u{1F3FF}\u{E0020}-\u{E007F}]$")
                .ok(),
            Regex::new(r"^\p{Mc}$").ok(),
            Regex::new(r"^\p{Extended_Pictographic}$").ok(),
            Regex::new(r"^[\u{0915}-\u{0939}\u{0958}-\u{095F}\u{0995}-\u{09B9}\u{09DC}-\u{09DF}\u{0A95}-\u{0AB9}\u{0B15}-\u{0B39}\u{0B5C}\u{0B5D}\u{0B5F}\u{0C15}-\u{0C39}\u{0D15}-\u{0D3A}]$").ok(),
        ]
    });
    let mut buf = [0u8; 4];
    set[idx]
        .as_ref()
        .is_some_and(|r| r.is_match(ch.encode_utf8(&mut buf)))
}

fn is_tag_block(ch: char) -> bool {
    matches!(ch as u32, 0xE0000..=0xE007F)
}

/// Checks whether `chars[start..end]` is a valid UTS #51 RGI emoji tag sequence.
pub fn is_valid_rgi_tag_sequence(chars: &[char], start: usize, end: usize) -> bool {
    if start == 0 || end <= start || end > chars.len() || chars[start - 1] != TAG_BASE {
        return false;
    }
    let Some(spec) = chars[start..end].strip_suffix(&[TAG_END]) else {
        return false;
    };
    let valid_spec = !spec.is_empty()
        && 2 + spec.len() <= 32
        && spec.iter().all(|&c| matches!(c as u32, 0xE0020..=0xE007E));
    if !valid_spec {
        return false;
    }
    let dec: String = spec
        .iter()
        .filter_map(|&c| char::from_u32(c as u32 - 0xE0000))
        .collect();
    RGI_TAG_SPECS.contains(&dec.as_str())
}

/// Returns `true` for disallowed `Cc`/`Cf`, bidi overrides/isolates, stray tags,
/// and legacy layout separators while keeping `\n`, `\t`, ZWNJ, ZWJ, LRM, RLM.
pub fn is_unsafe_char(ch: char) -> bool {
    !matches!(ch, '\n' | '\t' | ZWNJ | ZWJ | '\u{200E}' | '\u{200F}')
        && (ch.is_ascii_control() || (!ch.is_ascii() && re_hit(0, ch)))
}

fn hangul_join(prev: char, curr: char) -> bool {
    let cls = |c: char| match c as u32 {
        0x1100..=0x115F | 0xA960..=0xA97C => 1u8,
        0x1160..=0x11A7 | 0xD7B0..=0xD7C6 => 2,
        0x11A8..=0x11FF | 0xD7CB..=0xD7FB => 3,
        cp @ 0xAC00..=0xD7A3 => u8::from(!(cp - 0xAC00).is_multiple_of(28)) + 4,
        _ => 0,
    };
    matches!(
        (cls(prev), cls(curr)),
        (1, 1..=5) | (2 | 4, 2 | 3) | (3 | 5, 3)
    )
}

fn sequence_starts_chars(chars: &[char]) -> Vec<usize> {
    if chars.is_empty() {
        return Vec::new();
    }
    let is_ri = |c: char| matches!(c as u32, 0x1F1E6..=0x1F1FF);
    let mut starts = vec![0usize];
    let mut ep = u8::from(!chars[0].is_ascii() && re_hit(3, chars[0]));
    let mut ic = u8::from(!chars[0].is_ascii() && re_hit(4, chars[0]));
    let mut ri_odd = is_ri(chars[0]);
    for i in 1..chars.len() {
        let (prev, ch) = (chars[i - 1], chars[i]);
        let hi = !ch.is_ascii();
        let ext = hi && re_hit(1, ch);
        let is_ep = hi && re_hit(3, ch);
        let is_ic = hi && re_hit(4, ch);
        let ri = is_ri(ch);
        let same = ch == ZWJ
            || ext
            || (hi && re_hit(2, ch))
            || matches!(prev, ZWNJ | ZWJ)
            || (ep == 2 && is_ep)
            || (ri_odd && ri)
            || (ic == 2 && is_ic)
            || hangul_join(prev, ch);
        if !(prev == '\r' && ch == '\n') && (prev.is_control() || ch.is_control() || !same) {
            starts.push(i);
        }
        ep = match (is_ep || (ep == 1 && ext), ep == 1 && ch == ZWJ) {
            (true, _) => 1,
            (_, true) => 2,
            _ => 0,
        };
        ri_odd = ri && !ri_odd;
        ic = match (
            is_ic,
            ic > 0 && canonical_combining_class(ch) == 9,
            ic > 0 && (ext || ch == ZWJ),
        ) {
            (true, _, _) => 1,
            (_, true, _) => 2,
            (_, _, true) => ic,
            _ => 0,
        };
    }
    starts
}

/// Returns scalar-character indices where each UAX #29 + ZWNJ-bonded sequence begins.
pub fn sequence_starts(text: &str) -> Vec<usize> {
    sequence_starts_chars(&text.chars().collect::<Vec<_>>())
}

fn build_and_filter(raw: &str, opts: ProjectionOptions) -> (Vec<Tracked>, usize) {
    let mut tracked = Vec::with_capacity(raw.len());
    let (mut u16_pos, mut it) = (0usize, raw.chars().peekable());
    while let Some(ch) = it.next() {
        let crlf =
            (opts.single_line || opts.normalize_breaks) && ch == '\r' && it.peek() == Some(&'\n');
        let (mapped, units) = if crlf {
            it.next();
            (if opts.single_line { ' ' } else { '\n' }, 2)
        } else if opts.single_line && SINGLE_SEPS.contains(ch) {
            (' ', ch.len_utf16())
        } else if opts.normalize_breaks && BREAK_SEPS.contains(ch) {
            ('\n', ch.len_utf16())
        } else {
            (ch, ch.len_utf16())
        };
        tracked.push(Tracked(mapped, u16_pos, u16_pos + units, true));
        u16_pos += units;
    }
    let chars: Vec<char> = tracked.iter().map(|t| t.0).collect();
    let mut k = 0;
    while k < chars.len() {
        let s = k;
        while k < chars.len() && is_tag_block(chars[k]) {
            k += 1;
        }
        if k > s && !is_valid_rgi_tag_sequence(&chars, s, k) {
            tracked[s..k].iter_mut().for_each(|t| t.3 = false);
        }
        k += usize::from(k == s);
    }
    let (mut nl_run, mut prev_sp) = (0usize, false);
    let non_ascii_ok = |j: Option<usize>| {
        j.and_then(|k| chars.get(k))
            .is_some_and(|&c| c as u32 > 0x7F && !is_unsafe_char(c))
    };
    for (idx, item) in tracked.iter_mut().enumerate() {
        let ch = item.0;
        let bad_join = opts.strip_ascii_joiners
            && matches!(ch, ZWNJ | ZWJ)
            && !(non_ascii_ok(idx.checked_sub(1)) && non_ascii_ok(Some(idx + 1)));
        if !item.3 || (!is_tag_block(ch) && is_unsafe_char(ch)) || bad_join {
            item.3 = false;
            continue;
        }
        if opts.collapse_newlines {
            nl_run = if ch == '\n' { nl_run + 1 } else { 0 };
            item.3 = nl_run <= 2;
        } else if opts.single_line {
            item.3 = !(ch == ' ' && prev_sp);
            prev_sp = ch == ' ';
        }
    }
    if opts.trim {
        let lo = tracked.iter().position(|t| t.3 && !t.0.is_whitespace());
        let hi = tracked.iter().rposition(|t| t.3 && !t.0.is_whitespace());
        for (i, t) in tracked.iter_mut().enumerate() {
            t.3 &= lo.zip(hi).is_some_and(|(l, r)| i >= l && i <= r);
        }
    }
    (tracked, u16_pos)
}

fn apply_truncation(tracked: &mut [Tracked], opts: ProjectionOptions) -> (bool, &'static str) {
    let Some(limit) = opts.max_chars else {
        return (false, "");
    };
    let kept: Vec<usize> = (0..tracked.len()).filter(|&i| tracked[i].3).collect();
    if limit == 0 || kept.len() <= limit {
        if limit == 0 {
            kept.iter().for_each(|&i| tracked[i].3 = false);
        }
        let truncated = limit == 0 && !kept.is_empty();
        return (
            truncated,
            if truncated {
                opts.truncation_marker
            } else {
                ""
            },
        );
    }
    let m = opts.truncation_marker;
    let use_m = !m.is_empty() && limit >= m.chars().count();
    let marker = if use_m { m } else { ELLIPSIS_MARKER };
    let budget = limit.saturating_sub(marker.chars().count());
    let chars: Vec<char> = kept.iter().map(|&i| tracked[i].0).collect();
    let starts = sequence_starts_chars(&chars);
    let mut cut = starts.into_iter().rfind(|&s| s <= budget).unwrap_or(0);
    while cut > 0 && (chars[cut - 1].is_whitespace() || matches!(chars[cut - 1], ZWNJ | ZWJ)) {
        cut -= 1;
    }
    kept[cut..].iter().for_each(|&i| tracked[i].3 = false);
    (true, marker)
}

fn sanitize_map(m: &Map<String, Value>) -> Map<String, Value> {
    m.iter()
        .map(|(k, v)| (k.clone(), sanitize_meta(v)))
        .collect()
}

fn sanitize_meta(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(display_text(Some(s), DEFAULT_MAX_TEXT_CHARS)),
        Value::Array(xs) => Value::Array(xs.iter().map(sanitize_meta).collect()),
        Value::Object(m) => Value::Object(sanitize_map(m)),
        other => other.clone(),
    }
}

/// Rebases untrusted UTF-16 entity spans through `boundary_map` onto `content`,
/// clipping truncated spans and snapping inward off surrogate halves.
pub fn rebase_entities(
    entities: &[RawTextEntity],
    boundary_map: &[usize],
    content: &str,
) -> (Vec<TextEntity>, usize, usize) {
    if entities.is_empty() || content.is_empty() || boundary_map.is_empty() {
        return (Vec::new(), 0, entities.len());
    }
    let (orig_len, u16_len) = (boundary_map.len() - 1, content.encode_utf16().count());
    let mut valid = vec![false; u16_len + 1];
    valid[0] = true;
    let mut p = 0;
    for c in content.chars() {
        p += c.len_utf16();
        valid[p] = true;
    }
    let (mut retained, mut clipped, mut dropped) = (Vec::new(), 0usize, 0usize);
    for e in entities {
        let (rs, rl) = match (usize::try_from(e.offset), usize::try_from(e.length)) {
            (Ok(s), Ok(l)) if l > 0 && s < orig_len => (s, l),
            _ => {
                dropped += 1;
                continue;
            }
        };
        let re = rs.saturating_add(rl);
        let s0 = boundary_map[rs].min(u16_len);
        let e0 = boundary_map[re.min(orig_len)].min(u16_len);
        let s = (s0 + usize::from(!valid[s0])).min(u16_len);
        let end = e0.saturating_sub(usize::from(!valid[e0]));
        if end <= s {
            dropped += 1;
            continue;
        }
        clipped += usize::from(re > orig_len || (end - s) != rl);
        let kind = display_name(Some(&e.kind), DEFAULT_MAX_NAME_CHARS);
        let metadata = sanitize_map(&e.metadata);
        retained.push(TextEntity {
            kind,
            offset: s,
            length: end - s,
            metadata,
        });
    }
    (retained, clipped, dropped)
}

/// Projects `raw` text and its UTF-16 `entities` according to `options`.
pub fn project_text(
    raw: Option<&str>,
    entities: &[RawTextEntity],
    options: ProjectionOptions,
) -> TextFidelityResult {
    let source = raw.unwrap_or("");
    let (mut tracked, original_utf16_len) = build_and_filter(source, options);
    let (truncated, marker) = apply_truncation(&mut tracked, options);
    let mut content = String::new();
    let mut boundary_map = vec![0usize; original_utf16_len + 1];
    let mut out_u16 = 0usize;
    for t in &tracked {
        if t.3 {
            let u = t.0.len_utf16();
            for step in 0..t.2.saturating_sub(t.1) {
                boundary_map[t.1 + step] = out_u16 + step.min(u.saturating_sub(1));
            }
            content.push(t.0);
            out_u16 += u;
        } else {
            boundary_map[t.1..t.2].fill(out_u16);
        }
    }
    boundary_map[original_utf16_len] = out_u16;
    let empty = content.is_empty() && !truncated;
    let (text, entities, clipped_entities, dropped_entities) = if empty && options.empty_placeholder
    {
        (EMPTY_PLACEHOLDER.to_string(), Vec::new(), 0, entities.len())
    } else {
        let (kept, clipped, dropped) = rebase_entities(entities, &boundary_map, &content);
        if truncated && !marker.is_empty() {
            content.push_str(marker);
        }
        (content, kept, clipped, dropped)
    };
    let metadata = TextFidelityMetadata {
        modified: text != source,
        truncated,
        empty,
        original_utf16_len,
        content_utf16_len: out_u16,
        output_utf16_len: text.encode_utf16().count(),
        clipped_entities,
        dropped_entities,
    };
    TextFidelityResult {
        text,
        entities,
        metadata,
        boundary_map,
    }
}

pub fn fidelity_text(raw: Option<&str>) -> (String, Vec<usize>) {
    let r = project_text(raw, &[], ProjectionOptions::fidelity());
    (r.text, r.boundary_map)
}
pub fn display_name(raw: Option<&str>, max_length: usize) -> String {
    project_text(raw, &[], ProjectionOptions::display_name(max_length)).text
}
pub fn display_text_status(raw: Option<&str>, max_length: usize) -> (String, bool) {
    let r = project_text(raw, &[], ProjectionOptions::display_text(max_length));
    (r.text, r.metadata.truncated)
}
pub fn display_text(raw: Option<&str>, max_length: usize) -> String {
    display_text_status(raw, max_length).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PERSIAN: &str = "\u{0645}\u{06CC}\u{200C}\u{06A9}\u{0646}\u{062F}";
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    const SCOTLAND: &str = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}";

    fn ent(k: &str, o: i64, l: i64) -> RawTextEntity {
        RawTextEntity {
            kind: k.into(),
            offset: o,
            length: l,
            metadata: Map::new(),
        }
    }
    fn tag(spec: &str) -> String {
        let mut s = String::from(TAG_BASE);
        for c in spec.bytes() {
            s.extend(char::from_u32(0xE0000 + u32::from(c)));
        }
        s.push(TAG_END);
        s
    }

    #[test]
    fn unicode_fidelity_and_tags() {
        let f = |s: &str| fidelity_text(Some(s)).0;
        let ok = format!("{PERSIAN}|{FAMILY}|{SCOTLAND}|a\u{200E}\u{200F}\u{062F}|a\tb\n\nc");
        assert!(ok.split('|').all(|s| f(s) == s));
        assert!(["gbeng", "gbwls"].iter().all(|&s| f(&tag(s)) == tag(s)));
        let dirty = "a\0\u{200B}\u{202E}\u{2066}\u{FEFF}\u{2028}\u{2029}b";
        assert_eq!(f(dirty), "ab");
        let long = "a".repeat(40);
        let bad = ["us01", "GBSCT", "gb", &long];
        assert!(bad.iter().all(|s| !f(&tag(s)).chars().any(is_tag_block)));
        let stray = "\u{1F600}\u{E0067}\u{E007F}|\u{1F3F4}\u{E0067}|\u{1F3F4}\u{E007F}|a\u{E0001}b";
        assert!(stray.split('|').all(|s| !f(s).chars().any(is_tag_block)));
    }

    #[test]
    fn grapheme_truncation_breaks_and_empty() {
        let joined = format!(
            "\u{1F44D}\u{1F3FD}|\u{1F469}\u{1F3FB}\u{200D}\u{1F4BB}|\u{1100}\u{1161}\u{11A8}|\u{0915}\u{094D}\u{0937}|\u{06CC}\u{200C}\u{06A9}|\u{1F1EE}\u{1F1F7}|{FAMILY}|{SCOTLAND}"
        );
        let pfx = "y".repeat(20);
        for seq in joined.split('|') {
            assert_eq!(sequence_starts(seq), vec![0]);
            let txt = format!("{pfx}{seq}");
            for b in 20..=(txt.chars().count() + 2) {
                let out = display_name(Some(&txt), b);
                let body = out.strip_suffix('…').unwrap_or(&out);
                let tail = body.strip_prefix(&pfx).unwrap_or("");
                assert!(out.chars().count() <= b && !body.ends_with(ZWJ) && !body.ends_with(ZWNJ));
                assert!(tail.is_empty() || tail == seq);
            }
        }
        let opts = ProjectionOptions::sanitized(4096);
        let norm = project_text(Some("  a\r\n\r\n\r\n\r\nb\rc  "), &[], opts);
        assert_eq!(norm.text, "a\n\nb\nc");
        let e = project_text(Some(" \u{200B}\u{202E}\n\t "), &[ent("bold", 0, 3)], opts);
        assert!(e.text == EMPTY_PLACEHOLDER && e.entities.is_empty() && e.metadata.empty);
        let seps = "\r\n|\r|\n|\t|\x0B|\x0C|\u{85}|\u{2028}|\u{2029}";
        assert!(
            seps.split('|')
                .all(|s| display_name(Some(&format!("a{s}b")), 256) == "a b")
        );
        assert!(display_name(None, 256).is_empty() && display_name(Some("abc"), 0).is_empty());
        let (ok_t, ok_c) = display_text_status(Some("a\r\nb"), 4096);
        assert!(ok_t == "a\nb" && !ok_c);
        let (cut, tr) = display_text_status(Some(&"x".repeat(50)), 10);
        assert!(tr && cut.chars().count() == 10 && cut.ends_with('…'));
    }

    #[test]
    fn rebases_surrogate_and_truncated_entities() {
        let mut ce = ent("custom_emoji", 4, 2);
        ce.metadata.insert("document_id".into(), json!("99"));
        let spans = [ce, ent("bold", 5, 3), ent("italic", -1, 2)];
        let opts = ProjectionOptions::sanitized(4096);
        let r = project_text(Some("hi \u{202E}😀 world"), &spans, opts);
        assert_eq!(r.text, "hi 😀 world");
        assert_eq!((r.entities.len(), r.metadata.dropped_entities), (2, 1));
        assert_eq!((r.entities[0].offset, r.entities[0].length), (3, 2));
        assert_eq!((r.entities[1].offset, r.entities[1].length), (5, 2));
        let long = format!("{}😀{}", "a".repeat(20), "b".repeat(50));
        let cap_spans = [ent("bold", 10, 40), ent("italic", 40, 10)];
        let cap = project_text(Some(&long), &cap_spans, ProjectionOptions::sanitized(35));
        assert!(cap.metadata.truncated && cap.text.ends_with(TRUNCATED_MARKER));
        assert!(cap.text.chars().count() <= 35 && cap.entities.len() == 1);
        let end = cap.entities[0].offset + cap.entities[0].length;
        assert_eq!(end, cap.metadata.content_utf16_len);
        assert!(cap.metadata.content_utf16_len < cap.metadata.output_utf16_len);
    }
}
