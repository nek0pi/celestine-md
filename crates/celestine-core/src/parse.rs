use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Frontmatter {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: TagsField,
    /// Soft-read deprecated singular forms
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TagsField(pub Vec<String>);

impl<'de> Deserialize<'de> for TagsField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        match Option::<Raw>::deserialize(deserializer)? {
            None => Ok(TagsField(vec![])),
            Some(Raw::One(s)) => Ok(TagsField(vec![s])),
            Some(Raw::Many(v)) => Ok(TagsField(v)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    pub raw: String,
    pub target: String,
    pub heading: Option<String>,
    pub block: Option<String>,
    pub display: Option<String>,
    pub embed: bool,
    /// Byte offset in body (after frontmatter stripped)
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineLink {
    pub text: String,
    pub dest: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callout {
    pub kind: String,
    pub title: Option<String>,
    pub start_line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedNote {
    pub frontmatter: Frontmatter,
    pub body: String,
    pub headings: Vec<(u8, String, usize)>,
    pub wikilinks: Vec<Wikilink>,
    pub md_links: Vec<InlineLink>,
    pub tags: Vec<String>,
    pub callouts: Vec<Callout>,
}

fn wikilink_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"!\[\[([^\[\]]+)\]\]|\[\[([^\[\]]+)\]\]").expect("wikilink re"))
}

fn md_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([^\]]*)\]\(([^)]+)\)").expect("md link re"))
}

fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(^|[\s(])#([A-Za-z][\w/-]*)").expect("tag re"))
}

fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*$").expect("heading re"))
}

fn callout_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^>\s*\[!([^\]]+)\]([+-])?\s*(.*)$").expect("callout re"))
}

/// Split YAML frontmatter from body.
///
/// The block must open with a line that is exactly `---` and close with a line
/// that is exactly `---` or `...` (trailing whitespace / CR tolerated). Anything
/// else — a `----` rule, `---foo`, an unterminated block — is plain body.
pub fn split_frontmatter(content: &str) -> (Frontmatter, String) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(body_start) = frontmatter_span(content) else {
        return (Frontmatter::default(), content.to_string());
    };
    let (yaml_start, yaml_end, body_start) = body_start;
    let yaml = &content[yaml_start..yaml_end];
    let body = content[body_start..].to_string();
    let mut fm: Frontmatter = serde_yaml::from_str(yaml).unwrap_or_default();
    if fm.aliases.is_empty() {
        if let Some(a) = fm.alias.take() {
            fm.aliases.push(a);
        }
    }
    if fm.tags.0.is_empty() {
        if let Some(t) = fm.tag.take() {
            fm.tags.0.push(t);
        }
    }
    (fm, body)
}

/// `(yaml_start, yaml_end, body_start)` byte offsets, or `None` when the
/// content does not begin with a properly delimited frontmatter block.
fn frontmatter_span(content: &str) -> Option<(usize, usize, usize)> {
    let mut lines = line_spans(content);
    let (first_start, first_end, first_next) = lines.next()?;
    if content[first_start..first_end].trim_end() != "---" {
        return None;
    }
    for (start, end, next) in lines {
        let line = content[start..end].trim_end();
        if line == "---" || line == "..." {
            return Some((first_next, start, next));
        }
    }
    None
}

/// Yields `(start, end_excl_newline, start_of_next_line)` for each line.
fn line_spans(s: &str) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut pos = 0;
    std::iter::from_fn(move || {
        if pos >= s.len() {
            return None;
        }
        let start = pos;
        let rest = &s[start..];
        let (end, next) = match rest.find('\n') {
            Some(i) => (start + i, start + i + 1),
            None => (s.len(), s.len()),
        };
        pos = next;
        Some((start, end, next))
    })
}

/// Does `dest` start with a URI scheme (`mailto:`, `https://`, `obsidian://`…)?
/// A scheme is 2+ ASCII letters/digits/`+.-` starting with a letter, followed by
/// `:` and a non-space, so `Meeting: notes` and Windows drive letters don't count.
pub fn has_uri_scheme(dest: &str) -> bool {
    let bytes = dest.as_bytes();
    let Some(colon) = dest.find(':') else {
        return false;
    };
    if colon < 2 || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    if !bytes[..colon]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
    {
        return false;
    }
    match bytes.get(colon + 1) {
        Some(b) => !b.is_ascii_whitespace(),
        None => true,
    }
}

/// Replace the bytes of fenced code blocks (and, when `inline` is set, inline
/// code spans) with spaces, keeping newlines so byte offsets and line indices
/// of the result line up with `body`.
fn mask_code(body: &str, inline: bool) -> String {
    let mut out = body.as_bytes().to_vec();
    let blank = |out: &mut [u8], from: usize, to: usize| {
        for b in &mut out[from..to] {
            if *b != b'\n' && *b != b'\r' {
                *b = b' ';
            }
        }
    };

    // Fenced blocks: a line of up to 3 spaces then ``` or ~~~ (3+), closed by a
    // line with the same fence char at least as long and nothing else after it.
    let mut fence: Option<(u8, usize, usize)> = None; // (char, len, start offset)
    let mut fenced: Vec<(usize, usize)> = Vec::new();
    for (start, end, next) in line_spans(body) {
        let line = &body[start..end];
        let stripped = line.trim_start_matches(' ');
        if line.len() - stripped.len() > 3 {
            continue;
        }
        let first = stripped.as_bytes().first().copied();
        let Some(c @ (b'`' | b'~')) = first else {
            continue;
        };
        let run = stripped.bytes().take_while(|b| *b == c).count();
        if run < 3 {
            continue;
        }
        match fence {
            None => {
                // An opening backtick fence may not contain backticks in its info string.
                if c == b'`' && stripped[run..].contains('`') {
                    continue;
                }
                fence = Some((c, run, start));
            }
            Some((fc, flen, fstart)) if fc == c && run >= flen && stripped[run..].trim().is_empty() => {
                fenced.push((fstart, next));
                fence = None;
            }
            Some(_) => {}
        }
    }
    if let Some((_, _, fstart)) = fence {
        fenced.push((fstart, body.len()));
    }
    for (a, b) in &fenced {
        blank(&mut out, *a, *b);
    }

    if inline {
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'`' || fenced.iter().any(|(a, b)| i >= *a && i < *b) {
                i += 1;
                continue;
            }
            let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
            // A code span cannot leave its block: search up to the next blank
            // line, or only to the end of the line when it opens in a heading.
            let limit = inline_block_end(body, i);
            // Find the next backtick run of exactly the same length.
            let mut j = i + run;
            let mut close = None;
            while j < limit {
                if bytes[j] == b'`' {
                    let r = bytes[j..].iter().take_while(|b| **b == b'`').count();
                    if r == run {
                        close = Some(j);
                        break;
                    }
                    j += r;
                } else {
                    j += 1;
                }
            }
            match close {
                Some(j) => {
                    blank(&mut out, i, j + run);
                    i = j + run;
                }
                None => i += run,
            }
        }
    }

    String::from_utf8(out).expect("masking only writes ASCII spaces")
}

/// End (exclusive) of the Markdown block containing byte `pos`: the end of the
/// line for an ATX heading, otherwise the first blank line after `pos`.
fn inline_block_end(body: &str, pos: usize) -> usize {
    let line_start = body[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &body[line_start..];
    let line_end = line.find('\n').map(|i| line_start + i).unwrap_or(body.len());
    let head = line[..line_end - line_start].trim_start_matches(' ');
    let hashes = head.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && head[hashes..].starts_with([' ', '\t']) {
        return line_end;
    }
    let mut cursor = line_end;
    while cursor < body.len() {
        let rest = &body[cursor + 1..];
        let next_end = rest.find('\n').map(|i| cursor + 1 + i).unwrap_or(body.len());
        if body[cursor + 1..next_end].trim().is_empty() {
            return cursor;
        }
        cursor = next_end;
    }
    body.len()
}

fn parse_wikilink_inner(inner: &str) -> (String, Option<String>, Option<String>, Option<String>) {
    let (main, display) = match inner.split_once('|') {
        Some((m, d)) => (m.trim(), Some(d.trim().to_string())),
        None => (inner.trim(), None),
    };
    let (target, heading, block) = if let Some((t, after)) = main.split_once('#') {
        if let Some(block) = after.strip_prefix('^') {
            (t.trim().to_string(), None, Some(block.trim().to_string()))
        } else {
            // Nested headings: keep full heading path
            (t.trim().to_string(), Some(after.trim().to_string()), None)
        }
    } else {
        (main.to_string(), None, None)
    };
    let target = target
        .strip_suffix(".md")
        .unwrap_or(&target)
        .to_string();
    (target, heading, block, display)
}

pub fn extract_note_data(content: &str) -> ParsedNote {
    let (frontmatter, body) = split_frontmatter(content);
    // Fenced blocks hide headings/callouts; inline spans additionally hide links/tags.
    let block_masked = mask_code(&body, false);
    let masked = mask_code(&body, true);

    let mut headings = Vec::new();
    let mut callouts = Vec::new();
    for (line_idx, (start, end, _)) in line_spans(&block_masked).enumerate() {
        let line = &block_masked[start..end];
        if let Some(c) = heading_re().captures(line) {
            let level = c.get(1).unwrap().as_str().len() as u8;
            let text = c.get(2).unwrap().as_str().to_string();
            headings.push((level, text, line_idx));
        }
        if let Some(c) = callout_re().captures(line) {
            let kind = c.get(1).unwrap().as_str().trim().to_lowercase();
            let title = c.get(3).map(|m| m.as_str().trim().to_string()).filter(|s| !s.is_empty());
            callouts.push(Callout {
                kind,
                title,
                start_line: line_idx,
            });
        }
    }

    let mut wikilinks = Vec::new();
    for cap in wikilink_re().captures_iter(&masked) {
        let whole = cap.get(0).unwrap();
        let embed = whole.as_str().starts_with('!');
        let inner = cap
            .get(1)
            .or_else(|| cap.get(2))
            .unwrap()
            .as_str();
        let (target, heading, block, display) = parse_wikilink_inner(inner);
        wikilinks.push(Wikilink {
            raw: whole.as_str().to_string(),
            target,
            heading,
            block,
            display,
            embed,
            start: whole.start(),
            end: whole.end(),
        });
    }

    let mut md_links = Vec::new();
    for cap in md_link_re().captures_iter(&masked) {
        let whole = cap.get(0).unwrap();
        // Skip images that look like ![alt](url) — still track destination
        let text = cap.get(1).unwrap().as_str().to_string();
        let dest = cap.get(2).unwrap().as_str().to_string();
        // External destinations (http, mailto, obsidian, …) are not vault links.
        if has_uri_scheme(&dest) {
            continue;
        }
        md_links.push(InlineLink {
            text,
            dest,
            start: whole.start(),
            end: whole.end(),
        });
    }

    let mut tags = frontmatter.tags.0.clone();
    for cap in tag_re().captures_iter(&masked) {
        let t = cap.get(2).unwrap().as_str().to_string();
        if !tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            tags.push(t);
        }
    }

    ParsedNote {
        frontmatter,
        body,
        headings,
        wikilinks,
        md_links,
        tags,
        callouts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_and_wikilinks() {
        let src = r#"---
aliases:
  - Hello
tags: [a, b]
---
# Title

See [[Other Note|label]] and ![[Embed]] and [[Folder/Note#Heading]].
Also #inbox and [[Note#^block1]].
"#;
        let p = extract_note_data(src);
        assert_eq!(p.frontmatter.aliases, vec!["Hello"]);
        assert_eq!(p.tags, vec!["a", "b", "inbox"]);
        assert_eq!(p.headings.len(), 1);
        assert_eq!(p.wikilinks.len(), 4);
        assert!(p.wikilinks[1].embed);
        assert_eq!(p.wikilinks[2].heading.as_deref(), Some("Heading"));
        assert_eq!(p.wikilinks[3].block.as_deref(), Some("block1"));
    }
    #[test]
    fn frontmatter_requires_bare_opening_delimiter() {
        // `---foo` and `----` are not frontmatter openers; nothing may be cut.
        let p = extract_note_data("---foo\ntitle: x\n---\n# Kept\n");
        assert!(p.frontmatter.title.is_none());
        assert_eq!(p.headings.len(), 1, "heading after fake frontmatter must survive");
        assert_eq!(p.headings[0].1, "Kept");

        let p = extract_note_data("----\n# Kept\n----\nbody\n");
        assert_eq!(p.headings.len(), 1);
        assert!(p.body.contains("body"));
    }

    #[test]
    fn frontmatter_requires_bare_closing_delimiter() {
        // `---more` does not close the block, so there is no frontmatter at all.
        let p = extract_note_data("---\ntitle: x\n---more\n# Kept\n");
        assert!(p.frontmatter.title.is_none());
        assert_eq!(p.headings.len(), 1);
        assert!(p.body.starts_with("---\ntitle: x"));

        // YAML's `...` document-end marker also closes the block.
        let p = extract_note_data("---\ntitle: x\n...\n# Kept\n");
        assert_eq!(p.frontmatter.title.as_deref(), Some("x"));
        assert_eq!(p.headings.len(), 1);
        assert_eq!(p.body, "# Kept\n");
    }

    #[test]
    fn frontmatter_crlf_and_trailing_whitespace() {
        let p = extract_note_data("---\r\ntitle: x\r\n--- \r\n# Kept\r\n");
        assert_eq!(p.frontmatter.title.as_deref(), Some("x"));
        assert_eq!(p.headings.len(), 1);
    }

    #[test]
    fn fenced_code_is_not_parsed() {
        let src = "# Real\n```sh\n# comment\n[[Fake]] #faketag [fake](fake.md)\n```\n~~~\n# also fake\n~~~\n# Real2\n[[Real Link]] #realtag\n";
        let p = extract_note_data(src);
        let heads: Vec<_> = p.headings.iter().map(|h| h.1.as_str()).collect();
        assert_eq!(heads, vec!["Real", "Real2"]);
        assert_eq!(p.headings[1].2, 8, "line index of Real2 must be unaffected by masking");
        assert_eq!(p.wikilinks.len(), 1);
        assert_eq!(p.wikilinks[0].target, "Real Link");
        let wl = &p.wikilinks[0];
        assert_eq!(&p.body[wl.start..wl.end], "[[Real Link]]", "offsets must index the real body");
        assert!(p.md_links.is_empty());
        assert_eq!(p.tags, vec!["realtag"]);
        assert_eq!(p.body, src, "body must stay unmasked");
    }

    #[test]
    fn inline_code_is_not_parsed() {
        let src = "Use `[[Not A Link]]` and `#nottag` and ``[x](y.md)`` but [[Yes]] #yes\n";
        let p = extract_note_data(src);
        assert_eq!(p.wikilinks.len(), 1);
        assert_eq!(p.wikilinks[0].target, "Yes");
        assert_eq!(&p.body[p.wikilinks[0].start..p.wikilinks[0].end], "[[Yes]]");
        assert!(p.md_links.is_empty());
        assert_eq!(p.tags, vec!["yes"]);
    }

    #[test]
    fn heading_with_inline_code_keeps_text() {
        let p = extract_note_data("# Run `cargo test`\n");
        assert_eq!(p.headings[0].1, "Run `cargo test`");
    }

    #[test]
    fn unclosed_backtick_is_literal() {
        let p = extract_note_data("a ` b [[Link]]\n");
        assert_eq!(p.wikilinks.len(), 1);
    }

    #[test]
    fn stray_backtick_does_not_pair_across_paragraphs() {
        // CommonMark code spans never cross a blank line, so the literal
        // backtick in the first paragraph must not swallow the link and tag.
        let src = "A literal ` here.\n\n[[Real]] #real\n\nUse `code` now.\n";
        let p = extract_note_data(src);
        assert_eq!(p.wikilinks.len(), 1, "{:?}", p.wikilinks);
        assert_eq!(p.wikilinks[0].target, "Real");
        assert_eq!(p.tags, vec!["real"]);
    }

    #[test]
    fn backtick_in_heading_does_not_pair_with_next_line() {
        let p = extract_note_data("# Title `\n[[Real]] `x`\n");
        assert_eq!(p.wikilinks.len(), 1);
    }

    #[test]
    fn heading_only_wikilink_has_empty_target() {
        let p = extract_note_data("[[#Section]] and [[Note#Other]]\n");
        assert_eq!(p.wikilinks[0].target, "");
        assert_eq!(p.wikilinks[0].heading.as_deref(), Some("Section"));
        assert_eq!(p.wikilinks[1].target, "Note");
    }

    #[test]
    fn scheme_links_are_not_internal() {
        let p = extract_note_data("[m](mailto:x@y.z) [h](https://a.b) [f](ftp://a) [n](Note.md) [a](#anchor)\n");
        let dests: Vec<_> = p.md_links.iter().map(|l| l.dest.as_str()).collect();
        assert_eq!(dests, vec!["Note.md", "#anchor"]);
    }
}
