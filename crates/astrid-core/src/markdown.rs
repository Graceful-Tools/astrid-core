//! What a description, a comment or a chat message says, as blocks a screen can draw.
//!
//! Mirrors `astrid-web/lib/markdown.ts` — `renderMarkdownWithLinks` — which is what the web
//! draws a task description, a comment and a chat bubble through (task 11cfaf6d). A description
//! written on the web with any formatting read as line noise on Windows: `**bold**` with the
//! asterisks, `## heading` with the hashes, a URL nobody could click.
//!
//! The web's renderer is `marked` with GitHub-flavoured markdown and `breaks: true`, after Astrid's
//! three reference forms have been swapped out for pills. The rules this module follows are that
//! file's, read from it rather than remembered:
//!
//! - **Full GFM**: headings, emphasis, strikethrough, inline code and fenced blocks, ordered and
//!   bulleted lists with task checkboxes, block quotes, rules, tables, links.
//! - **A newline is a line break** (`breaks: true`), not a space.
//! - **Bare URLs are links** — `https://…`, and `www.` hosts, which the web upgrades to `https`.
//! - **`@[Name](userId)`, `#[List](listId)` and `![Task](taskId)` are pills**, not markdown. They
//!   come out BEFORE parsing, because `![…](…)` is image syntax and a parser that saw it would
//!   draw a task reference as a broken picture.
//! - **Only `http`, `https` and `mailto` links keep their address.** Anything else — a
//!   `javascript:` in particular — is text, which is what the web's sanitiser leaves of it.
//! - **HTML is text.** The web strips tags it does not allow and keeps what was inside them.
//!
//! What comes out is a tree of [`Block`]s of [`Inline`]s rather than HTML, because a WinUI
//! shell draws paragraphs and runs, not markup. The shell decides fonts and colours; nothing
//! here does. It does not decide what `**` means either — that is this file's, once.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;

/// One block of a rendered text, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Block {
    Paragraph {
        inlines: Vec<Inline>,
    },
    Heading {
        /// 1 through 6.
        level: u8,
        inlines: Vec<Inline>,
    },
    /// A fenced or indented code block. Drawn monospace, and it scrolls sideways rather than
    /// widening whatever it is in (the web's `.prose pre`).
    Code {
        language: Option<String>,
        text: String,
    },
    List {
        ordered: bool,
        /// The first number of an ordered list. `1` for a bulleted one.
        start: u64,
        items: Vec<ListItem>,
    },
    Quote {
        blocks: Vec<Block>,
    },
    Rule,
    Table {
        /// One per column: `left`, `center`, `right` or `none`.
        alignments: Vec<String>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

/// One item of a list: its own blocks, and a checkbox if it is a task-list item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    /// `Some` for a `- [ ]` / `- [x]` item; `None` for an ordinary one.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// One run inside a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Inline {
    Text {
        text: String,
        bold: bool,
        italic: bool,
        strike: bool,
        /// An inline code span.
        code: bool,
        /// Where the run goes when clicked, when it goes anywhere: an absolute `http`, `https` or
        /// `mailto` address.
        link: Option<String>,
    },
    /// One of Astrid's pills: a person, a list or a task, by id.
    Reference {
        reference: ReferenceKind,
        label: String,
        id: String,
    },
    LineBreak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceKind {
    User,
    List,
    Task,
}

/// Render markdown to blocks, the way the web renders it to HTML.
pub fn render(text: &str) -> Vec<Block> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let (with_sentinels, references) = extract_references(text);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut builder = Builder::new(references);
    for event in Parser::new_ext(&with_sentinels, options) {
        builder.event(event);
    }
    builder.finish()
}

// ── References ─────────────────────────────────────────────────────────────────────────────

/// Private-use code points standing in for a reference while the parser runs, as the web does
/// (`REF_OPEN` / `REF_CLOSE` in `lib/markdown.ts`). The parser leaves them alone, and nobody
/// types U+E000.
const REF_OPEN: char = '\u{E000}';
const REF_CLOSE: char = '\u{E001}';

#[derive(Debug, Clone)]
struct Reference {
    kind: ReferenceKind,
    label: String,
    id: String,
    /// What was typed, for the one place a pill cannot be drawn: inside a code block.
    source: String,
}

/// Swap the three reference forms out for sentinels, in the order the web extracts them.
fn extract_references(text: &str) -> (String, Vec<Reference>) {
    let mut references = Vec::new();
    let mut out = text.to_string();
    for (sigil, kind) in [
        ('@', ReferenceKind::User),
        ('#', ReferenceKind::List),
        ('!', ReferenceKind::Task),
    ] {
        out = replace_references(&out, sigil, kind, &mut references);
    }
    (out, references)
}

/// Replace every `<sigil>[label](id)` in `text` with a sentinel, recording the reference.
///
/// The web's pattern is `\[([^\]]+)\]\(([^)]+)\)` after the sigil: a non-empty label with no
/// `]` in it, a non-empty id with no `)` in it.
fn replace_references(
    text: &str,
    sigil: char,
    kind: ReferenceKind,
    references: &mut Vec<Reference>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(sigil) {
        let (before, from_sigil) = rest.split_at(at);
        out.push_str(before);
        match parse_reference(from_sigil) {
            Some((label, id, consumed)) => {
                references.push(Reference {
                    kind,
                    label: label.to_string(),
                    id: id.to_string(),
                    source: from_sigil[..consumed].to_string(),
                });
                out.push(REF_OPEN);
                out.push_str(&(references.len() - 1).to_string());
                out.push(REF_CLOSE);
                rest = &from_sigil[consumed..];
            }
            None => {
                let sigil_len = sigil.len_utf8();
                out.push_str(&from_sigil[..sigil_len]);
                rest = &from_sigil[sigil_len..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `<sigil>[label](id)` at the start of `text`: the label, the id, and how many bytes it took.
fn parse_reference(text: &str) -> Option<(&str, &str, usize)> {
    let mut chars = text.char_indices();
    let (_, sigil) = chars.next()?;
    let after_sigil = sigil.len_utf8();
    if !text[after_sigil..].starts_with('[') {
        return None;
    }
    let label_start = after_sigil + 1;
    let label_end = label_start + text[label_start..].find(']')?;
    let label = &text[label_start..label_end];
    if label.is_empty() || !text[label_end + 1..].starts_with('(') {
        return None;
    }
    let id_start = label_end + 2;
    let id_end = id_start + text[id_start..].find(')')?;
    let id = &text[id_start..id_end];
    if id.is_empty() {
        return None;
    }
    Some((label, id, id_end + 1))
}

// ── Links ──────────────────────────────────────────────────────────────────────────────────

/// The web's origin, for a link written relative to it (`/lists/…`, `/u/…`).
const APP_ORIGIN: &str = "https://astrid.cc";

/// The address a link keeps, or `None` when the web's sanitiser would have dropped it.
///
/// `http`, `https` and `mailto` survive. A relative path is the app's own page. Anything else —
/// `javascript:`, `data:`, an unparseable string — is text.
fn safe_link(destination: &str) -> Option<String> {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
    {
        return Some(trimmed.to_string());
    }
    if trimmed.starts_with('/') && !trimmed.starts_with("//") {
        return Some(format!("{APP_ORIGIN}{trimmed}"));
    }
    if lower.starts_with("www.") {
        return Some(format!("https://{trimmed}"));
    }
    if lower.contains(':') {
        // A scheme this client does not follow.
        return None;
    }
    None
}

/// Bare URLs in plain text, as GFM's autolink extension finds them.
///
/// `https://…` and `http://…` from a word boundary, and `www.` hosts, which `marked` links as
/// `http://` and the web then upgrades to `https://`. Trailing `?!.,:*_~` are not part of the
/// address, and neither is a `)` that closes nothing inside it — the two rules the spec states.
/// An email address becomes a `mailto:` link, as GFM does.
fn split_autolinks(text: &str) -> Vec<(String, Option<String>)> {
    let mut parts: Vec<(String, Option<String>)> = Vec::new();
    let mut plain = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        let at_boundary = i == 0 || !is_word_byte(bytes[i - 1]);
        let candidate = if at_boundary { autolink_at(rest) } else { None };
        if let Some((length, href)) = candidate {
            if !plain.is_empty() {
                parts.push((std::mem::take(&mut plain), None));
            }
            parts.push((rest[..length].to_string(), Some(href)));
            i += length;
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        plain.push(ch);
        i += ch.len_utf8();
    }
    if !plain.is_empty() {
        parts.push((plain, None));
    }
    parts
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'/' || byte == b'.'
}

/// An autolink starting exactly here: how much of `rest` it covers, and where it points.
fn autolink_at(rest: &str) -> Option<(usize, String)> {
    let lower = rest.to_ascii_lowercase();
    let (scheme_len, upgrade_www) = if lower.starts_with("https://") {
        (8, false)
    } else if lower.starts_with("http://") {
        (7, false)
    } else if lower.starts_with("www.") {
        (0, true)
    } else {
        return email_at(rest);
    };
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(rest.len());
    let mut length = end;
    // Trailing punctuation is prose, not address.
    while length > scheme_len {
        let last = rest[..length].chars().next_back().unwrap();
        if "?!.,:*_~".contains(last) {
            length -= last.len_utf8();
        } else if last == ')' {
            let opened = rest[..length].matches('(').count();
            let closed = rest[..length].matches(')').count();
            if closed > opened {
                length -= 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    // A scheme with nothing after it is not a link.
    if length <= scheme_len + if upgrade_www { 4 } else { 0 } {
        return None;
    }
    let raw = &rest[..length];
    let href = if upgrade_www {
        format!("https://{raw}")
    } else {
        raw.to_string()
    };
    Some((length, href))
}

/// `name@host.tld` starting here, the way GFM recognises one.
fn email_at(rest: &str) -> Option<(usize, String)> {
    let at = rest.find('@')?;
    let local = &rest[..at];
    if local.is_empty()
        || !local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
        || local.contains(' ')
    {
        return None;
    }
    // The local part must run right up to the `@` from where we started.
    let domain_start = at + 1;
    let domain_end = rest[domain_start..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .map(|offset| domain_start + offset)
        .unwrap_or(rest.len());
    let mut domain = &rest[domain_start..domain_end];
    while domain.ends_with('.') || domain.ends_with('-') {
        domain = &domain[..domain.len() - 1];
    }
    if !domain.contains('.') || domain.is_empty() {
        return None;
    }
    let length = domain_start + domain.len();
    Some((length, format!("mailto:{}", &rest[..length])))
}

/// What is left of HTML the web would not allow: its text.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

// ── The builder ────────────────────────────────────────────────────────────────────────────

/// Something being built, innermost last.
enum Frame {
    /// A container of blocks: the document, a quote, or a list item's body.
    Blocks(Vec<Block>),
    Paragraph {
        inlines: Vec<Inline>,
        /// Opened by text arriving where no paragraph was declared — a tight list item — and
        /// closed when the next block starts or the item ends.
        implicit: bool,
    },
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Item {
        checked: Option<bool>,
        blocks: Vec<Block>,
    },
    Table {
        alignments: Vec<String>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        in_head: bool,
    },
    Row(Vec<Vec<Inline>>),
    Cell(Vec<Inline>),
    /// Raw HTML, which the web strips to its text.
    Html(String),
}

struct Builder {
    stack: Vec<Frame>,
    references: Vec<Reference>,
    bold: usize,
    italic: usize,
    strike: usize,
    links: Vec<Option<String>>,
    /// Text the parser has handed over that nothing else has interrupted yet.
    ///
    /// The parser splits text at every character that could have meant something — `[`, `_`,
    /// `(` — so `https://google.com]` arrives as three pieces, and a URL scanned one piece at a
    /// time stops where the parser stopped rather than where GFM says the address ends.
    /// Consecutive text is gathered and scanned once, when something else arrives.
    pending: String,
}

impl Builder {
    fn new(references: Vec<Reference>) -> Self {
        Self {
            stack: vec![Frame::Blocks(Vec::new())],
            references,
            bold: 0,
            italic: 0,
            strike: 0,
            links: Vec::new(),
            pending: String::new(),
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush_text();
        self.close_implicit_paragraph();
        while self.stack.len() > 1 {
            self.end_top();
        }
        match self.stack.pop() {
            Some(Frame::Blocks(blocks)) => blocks,
            _ => Vec::new(),
        }
    }

    fn event(&mut self, event: Event<'_>) {
        if let Event::Text(text) = &event {
            self.pending.push_str(text);
            return;
        }
        self.flush_text();
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(_) => unreachable!("gathered above"),
            Event::Code(code) => self.code_span(&code),
            Event::Html(html) | Event::InlineHtml(html) => self.html(&html),
            Event::SoftBreak | Event::HardBreak => {
                // `breaks: true`: a newline is a line break, not a space.
                self.inlines().push(Inline::LineBreak);
            }
            Event::Rule => {
                self.close_implicit_paragraph();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { checked: slot, .. }) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|frame| matches!(frame, Frame::Item { .. }))
                {
                    *slot = Some(checked);
                }
            }
            Event::FootnoteReference(name) => self.text(&format!("[^{name}]")),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.close_implicit_paragraph();
                self.stack.push(Frame::Paragraph {
                    inlines: Vec::new(),
                    implicit: false,
                });
            }
            Tag::Heading { level, .. } => {
                self.close_implicit_paragraph();
                self.stack.push(Frame::Heading {
                    level: heading_level(level),
                    inlines: Vec::new(),
                });
            }
            Tag::BlockQuote(_) => {
                self.close_implicit_paragraph();
                self.stack.push(Frame::Blocks(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.close_implicit_paragraph();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let language = info.split_whitespace().next().unwrap_or("").to_string();
                        (!language.is_empty()).then_some(language)
                    }
                    CodeBlockKind::Indented => None,
                };
                self.stack.push(Frame::Code {
                    language,
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                self.close_implicit_paragraph();
                self.stack.push(Frame::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.stack.push(Frame::Item {
                    checked: None,
                    blocks: Vec::new(),
                });
            }
            Tag::Table(alignments) => {
                self.close_implicit_paragraph();
                self.stack.push(Frame::Table {
                    alignments: alignments
                        .iter()
                        .map(|a| alignment_name(*a).to_string())
                        .collect(),
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Frame::Table { in_head, .. }) = self.stack.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => self.stack.push(Frame::Row(Vec::new())),
            Tag::TableCell => self.stack.push(Frame::Cell(Vec::new())),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.links.push(safe_link(&dest_url)),
            // Every `![…](…)` was taken out as a task reference before parsing, so an image here
            // is one with an empty label or id — drawn as its text, which is what is left of it.
            Tag::Image { .. } => self.links.push(None),
            Tag::HtmlBlock => {
                self.close_implicit_paragraph();
                self.stack.push(Frame::Html(String::new()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table { in_head, .. }) = self.stack.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Table
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::HtmlBlock => {
                self.close_implicit_paragraph();
                self.end_top();
            }
            _ => {}
        }
    }

    /// Close whatever is innermost and hand it to its parent.
    fn end_top(&mut self) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match frame {
            Frame::Blocks(blocks) => self.push_block(Block::Quote { blocks }),
            Frame::Paragraph { inlines, .. } => {
                if !inlines.is_empty() {
                    self.push_block(Block::Paragraph { inlines });
                }
            }
            Frame::Heading { level, inlines } => self.push_block(Block::Heading { level, inlines }),
            Frame::Code { language, text } => self.push_block(Block::Code { language, text }),
            Frame::List {
                ordered,
                start,
                items,
            } => self.push_block(Block::List {
                ordered,
                start,
                items,
            }),
            Frame::Item { checked, blocks } => {
                if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                    items.push(ListItem { checked, blocks });
                }
            }
            Frame::Table {
                alignments,
                header,
                rows,
                ..
            } => self.push_block(Block::Table {
                alignments,
                header,
                rows,
            }),
            Frame::Row(cells) => {
                if let Some(Frame::Table { rows, .. }) = self.stack.last_mut() {
                    rows.push(cells);
                }
            }
            Frame::Cell(inlines) => match self.stack.last_mut() {
                Some(Frame::Row(cells)) => cells.push(inlines),
                Some(Frame::Table {
                    header, in_head, ..
                }) if *in_head => header.push(inlines),
                _ => {}
            },
            Frame::Html(html) => {
                let text = strip_tags(&html);
                if !text.trim().is_empty() {
                    let mut inlines = Vec::new();
                    self.push_text_into(&mut inlines, text.trim_end_matches('\n'));
                    self.push_block(Block::Paragraph { inlines });
                }
            }
        }
    }

    /// Add a finished block to the innermost container.
    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Blocks(blocks)) | Some(Frame::Item { blocks, .. }) => blocks.push(block),
            // A block where only inlines can go — a rule inside a heading, say. The parser does
            // not produce these; dropping one is safer than corrupting the tree.
            _ => {}
        }
    }

    /// The inline run being written to, opening a paragraph if nothing is open.
    fn inlines(&mut self) -> &mut Vec<Inline> {
        let needs_paragraph = !matches!(
            self.stack.last(),
            Some(Frame::Paragraph { .. }) | Some(Frame::Heading { .. }) | Some(Frame::Cell(_))
        );
        if needs_paragraph {
            self.stack.push(Frame::Paragraph {
                inlines: Vec::new(),
                implicit: true,
            });
        }
        match self.stack.last_mut() {
            Some(Frame::Paragraph { inlines, .. })
            | Some(Frame::Heading { inlines, .. })
            | Some(Frame::Cell(inlines)) => inlines,
            _ => unreachable!("a paragraph was just pushed"),
        }
    }

    fn close_implicit_paragraph(&mut self) {
        if matches!(
            self.stack.last(),
            Some(Frame::Paragraph { implicit: true, .. })
        ) {
            self.end_top();
        }
    }

    /// Hand the gathered text to whatever is open.
    fn flush_text(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending);
        self.text(&text);
    }

    fn text(&mut self, text: &str) {
        if matches!(self.stack.last(), Some(Frame::Code { .. })) {
            let restored = self.restore_sources(text);
            if let Some(Frame::Code { text: body, .. }) = self.stack.last_mut() {
                body.push_str(&restored);
            }
            return;
        }
        match self.stack.last_mut() {
            Some(Frame::Html(html)) => html.push_str(text),
            _ => {
                let mut inlines = std::mem::take(self.inlines());
                self.push_text_into(&mut inlines, text);
                *self.inlines() = inlines;
            }
        }
    }

    fn code_span(&mut self, code: &str) {
        let mut inlines = std::mem::take(self.inlines());
        self.push_runs(&mut inlines, code, true, None);
        *self.inlines() = inlines;
    }

    fn html(&mut self, html: &str) {
        match self.stack.last_mut() {
            Some(Frame::Html(body)) => body.push_str(html),
            Some(Frame::Code { text, .. }) => text.push_str(html),
            _ => {
                let text = strip_tags(html);
                if !text.is_empty() {
                    let mut inlines = std::mem::take(self.inlines());
                    self.push_text_into(&mut inlines, &text);
                    *self.inlines() = inlines;
                }
            }
        }
    }

    /// Plain text into runs: references become pills, bare URLs become links, the rest is a run
    /// in the current style.
    fn push_text_into(&mut self, inlines: &mut Vec<Inline>, text: &str) {
        let inside_link = self.links.last().is_some();
        let mut rest = text;
        while let Some(open) = rest.find(REF_OPEN) {
            let (before, from_open) = rest.split_at(open);
            self.push_plain(inlines, before, inside_link);
            match from_open[REF_OPEN.len_utf8()..].find(REF_CLOSE) {
                Some(close) => {
                    let index = &from_open[REF_OPEN.len_utf8()..REF_OPEN.len_utf8() + close];
                    if let Some(reference) = index
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| self.references.get(i))
                    {
                        inlines.push(Inline::Reference {
                            reference: reference.kind,
                            label: reference.label.clone(),
                            id: reference.id.clone(),
                        });
                    }
                    rest = &from_open[REF_OPEN.len_utf8() + close + REF_CLOSE.len_utf8()..];
                }
                None => {
                    rest = "";
                }
            }
        }
        self.push_plain(inlines, rest, inside_link);
    }

    fn push_plain(&mut self, inlines: &mut Vec<Inline>, text: &str, inside_link: bool) {
        if text.is_empty() {
            return;
        }
        if inside_link {
            let link = self.links.last().cloned().flatten();
            self.push_runs(inlines, text, false, link);
            return;
        }
        for (part, href) in split_autolinks(text) {
            self.push_runs(inlines, &part, false, href);
        }
    }

    /// One run in the current style, merged into the previous one when the style is the same —
    /// the parser hands text over in pieces, and a screen drawing a run per piece would break
    /// a word at every entity.
    fn push_runs(
        &mut self,
        inlines: &mut Vec<Inline>,
        text: &str,
        code: bool,
        link: Option<String>,
    ) {
        if text.is_empty() {
            return;
        }
        let bold = self.bold > 0;
        let italic = self.italic > 0;
        let strike = self.strike > 0;
        if let Some(Inline::Text {
            text: previous,
            bold: b,
            italic: i,
            strike: s,
            code: c,
            link: l,
        }) = inlines.last_mut()
        {
            if *b == bold && *i == italic && *s == strike && *c == code && *l == link {
                previous.push_str(text);
                return;
            }
        }
        inlines.push(Inline::Text {
            text: text.to_string(),
            bold,
            italic,
            strike,
            code,
            link,
        });
    }

    /// Put the typed form of every reference back — for code blocks, where there are no pills.
    fn restore_sources(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find(REF_OPEN) {
            out.push_str(&rest[..open]);
            let after = &rest[open + REF_OPEN.len_utf8()..];
            match after.find(REF_CLOSE) {
                Some(close) => {
                    if let Some(reference) = after[..close]
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| self.references.get(i))
                    {
                        out.push_str(&reference.source);
                    }
                    rest = &after[close + REF_CLOSE.len_utf8()..];
                }
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alignment_name(alignment: Alignment) -> &'static str {
    match alignment {
        Alignment::None => "none",
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Inline {
        Inline::Text {
            text: text.to_string(),
            bold: false,
            italic: false,
            strike: false,
            code: false,
            link: None,
        }
    }

    fn styled(text: &str, bold: bool, italic: bool) -> Inline {
        Inline::Text {
            text: text.to_string(),
            bold,
            italic,
            strike: false,
            code: false,
            link: None,
        }
    }

    fn linked(text: &str, href: &str) -> Inline {
        Inline::Text {
            text: text.to_string(),
            bold: false,
            italic: false,
            strike: false,
            code: false,
            link: Some(href.to_string()),
        }
    }

    /// The description the task was filed with, rendered exactly as the web renders it: the
    /// hashes stay (CommonMark wants a space after them), the emphasis takes, every newline is a
    /// break, and the bare URL is a link — right up to the `]`, which GFM counts as part of it
    /// (task 11cfaf6d).
    #[test]
    fn the_reported_description_renders_as_the_web_renders_it_task_11cfaf6d() {
        let blocks = render("##title\n**bold**\n*italics*\n(link)[https://google.com]");
        assert_eq!(
            blocks,
            vec![Block::Paragraph {
                inlines: vec![
                    plain("##title"),
                    Inline::LineBreak,
                    styled("bold", true, false),
                    Inline::LineBreak,
                    styled("italics", false, true),
                    Inline::LineBreak,
                    plain("(link)["),
                    linked("https://google.com]", "https://google.com]"),
                ]
            }]
        );
    }

    #[test]
    fn a_heading_needs_its_space_and_then_is_one() {
        assert_eq!(
            render("## Plan"),
            vec![Block::Heading {
                level: 2,
                inlines: vec![plain("Plan")]
            }]
        );
        assert_eq!(
            render("### Deeper\nbody"),
            vec![
                Block::Heading {
                    level: 3,
                    inlines: vec![plain("Deeper")]
                },
                Block::Paragraph {
                    inlines: vec![plain("body")]
                }
            ]
        );
    }

    #[test]
    fn a_markdown_link_keeps_its_address_and_an_unsafe_one_loses_it() {
        assert_eq!(
            render("see [the docs](https://astrid.cc/docs) now"),
            vec![Block::Paragraph {
                inlines: vec![
                    plain("see "),
                    linked("the docs", "https://astrid.cc/docs"),
                    plain(" now"),
                ]
            }]
        );
        assert_eq!(
            render("[boom](javascript:alert(1))"),
            vec![Block::Paragraph {
                inlines: vec![plain("boom")]
            }]
        );
        assert_eq!(
            render("[home](/lists/abc)"),
            vec![Block::Paragraph {
                inlines: vec![linked("home", "https://astrid.cc/lists/abc")]
            }]
        );
    }

    /// `www.` hosts are linked and upgraded to https, as the web upgrades them; trailing
    /// punctuation stays prose.
    #[test]
    fn bare_urls_are_links() {
        assert_eq!(
            render("visit www.example.com, or https://astrid.cc/x. Mail me@example.org!"),
            vec![Block::Paragraph {
                inlines: vec![
                    plain("visit "),
                    linked("www.example.com", "https://www.example.com"),
                    plain(", or "),
                    linked("https://astrid.cc/x", "https://astrid.cc/x"),
                    plain(". Mail "),
                    linked("me@example.org", "mailto:me@example.org"),
                    plain("!"),
                ]
            }]
        );
    }

    #[test]
    fn a_wrapped_url_keeps_its_closing_bracket_only_when_it_opened_one() {
        assert_eq!(
            render("(https://en.wikipedia.org/wiki/Rust_(language))"),
            vec![Block::Paragraph {
                inlines: vec![
                    plain("("),
                    linked(
                        "https://en.wikipedia.org/wiki/Rust_(language)",
                        "https://en.wikipedia.org/wiki/Rust_(language)"
                    ),
                    plain(")"),
                ]
            }]
        );
    }

    /// The three reference forms are pills, and `![…](…)` in particular is NOT an image.
    #[test]
    fn astrid_references_are_pills_not_markdown() {
        assert_eq!(
            render("ask @[Jon Paris](u1) about #[Health](l1) and ![Buy milk](t1)."),
            vec![Block::Paragraph {
                inlines: vec![
                    plain("ask "),
                    Inline::Reference {
                        reference: ReferenceKind::User,
                        label: "Jon Paris".into(),
                        id: "u1".into()
                    },
                    plain(" about "),
                    Inline::Reference {
                        reference: ReferenceKind::List,
                        label: "Health".into(),
                        id: "l1".into()
                    },
                    plain(" and "),
                    Inline::Reference {
                        reference: ReferenceKind::Task,
                        label: "Buy milk".into(),
                        id: "t1".into()
                    },
                    plain("."),
                ]
            }]
        );
    }

    /// A pill inside a code block cannot be drawn, so the typed form comes back.
    #[test]
    fn a_reference_in_a_code_block_is_shown_as_typed() {
        assert_eq!(
            render("```\n![Buy milk](t1)\n```"),
            vec![Block::Code {
                language: None,
                text: "![Buy milk](t1)\n".into()
            }]
        );
    }

    #[test]
    fn a_fenced_block_keeps_its_language_and_its_text() {
        assert_eq!(
            render("```js\nlet x = 1;\nlet y = 2;\n```"),
            vec![Block::Code {
                language: Some("js".into()),
                text: "let x = 1;\nlet y = 2;\n".into()
            }]
        );
    }

    #[test]
    fn strikethrough_and_inline_code() {
        assert_eq!(
            render("~~gone~~ and `code`"),
            vec![Block::Paragraph {
                inlines: vec![
                    Inline::Text {
                        text: "gone".into(),
                        bold: false,
                        italic: false,
                        strike: true,
                        code: false,
                        link: None
                    },
                    plain(" and "),
                    Inline::Text {
                        text: "code".into(),
                        bold: false,
                        italic: false,
                        strike: false,
                        code: true,
                        link: None
                    },
                ]
            }]
        );
    }

    #[test]
    fn lists_bulleted_ordered_and_task() {
        let blocks = render("- one\n- [x] two\n- [ ] three\n\n3. a\n4. b");
        assert_eq!(
            blocks,
            vec![
                Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![
                        ListItem {
                            checked: None,
                            blocks: vec![Block::Paragraph {
                                inlines: vec![plain("one")]
                            }]
                        },
                        ListItem {
                            checked: Some(true),
                            blocks: vec![Block::Paragraph {
                                inlines: vec![plain("two")]
                            }]
                        },
                        ListItem {
                            checked: Some(false),
                            blocks: vec![Block::Paragraph {
                                inlines: vec![plain("three")]
                            }]
                        },
                    ]
                },
                Block::List {
                    ordered: true,
                    start: 3,
                    items: vec![
                        ListItem {
                            checked: None,
                            blocks: vec![Block::Paragraph {
                                inlines: vec![plain("a")]
                            }]
                        },
                        ListItem {
                            checked: None,
                            blocks: vec![Block::Paragraph {
                                inlines: vec![plain("b")]
                            }]
                        },
                    ]
                },
            ]
        );
    }

    #[test]
    fn a_nested_list_sits_inside_its_item() {
        let blocks = render("- outer\n  - inner");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected a list");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].blocks.len(), 2, "the text, then the nested list");
        assert!(matches!(items[0].blocks[1], Block::List { .. }));
    }

    #[test]
    fn quotes_rules_and_tables() {
        let blocks = render("> quote\n\n---\n\n| a | b |\n|---|--:|\n| 1 | 2 |");
        assert_eq!(
            blocks,
            vec![
                Block::Quote {
                    blocks: vec![Block::Paragraph {
                        inlines: vec![plain("quote")]
                    }]
                },
                Block::Rule,
                Block::Table {
                    alignments: vec!["none".into(), "right".into()],
                    header: vec![vec![plain("a")], vec![plain("b")]],
                    rows: vec![vec![vec![plain("1")], vec![plain("2")]]],
                },
            ]
        );
    }

    /// The web strips tags it does not allow and keeps their text.
    #[test]
    fn html_is_its_text() {
        assert_eq!(
            render("a <b>bold</b> claim"),
            vec![Block::Paragraph {
                inlines: vec![plain("a bold claim")]
            }]
        );
        assert_eq!(
            render("<div>\nblock\n</div>"),
            vec![Block::Paragraph {
                inlines: vec![plain("\nblock")]
            }]
        );
    }

    #[test]
    fn nothing_renders_as_nothing() {
        assert_eq!(render(""), Vec::<Block>::new());
        assert_eq!(render("   \n  "), Vec::<Block>::new());
    }

    /// The wire shape the shell reads: a `kind` on every block and inline.
    #[test]
    fn the_wire_shape_tags_every_node() {
        let json = serde_json::to_value(render("**hi** @[Jon](u1)")).unwrap();
        assert_eq!(json[0]["kind"], "paragraph");
        assert_eq!(json[0]["inlines"][0]["kind"], "text");
        assert_eq!(json[0]["inlines"][0]["bold"], true);
        assert_eq!(json[0]["inlines"][2]["kind"], "reference");
        assert_eq!(json[0]["inlines"][2]["reference"], "user");
    }
}
