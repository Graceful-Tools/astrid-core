//! The identifier format, and task ids written in prose.
//!
//! Mirrors astrid-web's `lib/task-identifier-core.ts` (the format) and
//! `lib/task-identifier-links.ts` (the autolinker), locked by the `parse` and `autolink` halves of
//! `contracts/fixtures/task-identifiers.json`. The spec is `docs/specs/TASK_IDENTIFIERS.md` §5.
//! Where an id is *shown* is the other half, and lives in [`crate::rows::identifier`].
//!
//! **The server is the only minter.** Nothing here derives an id; it recognises one somebody else
//! wrote. [`parse_identifier`] is what a picker uses to tell `AWTD-1007` typed into a search box
//! from a title, and [`find_links`] turns `AWTD-1007` in a comment into something clickable.
//!
//! Three rules earn their keep, and each is a case in the fixture:
//!
//! - **Only the reader's own project keys link.** Without that, `UTF-8` and `COVID-19` become task
//!   references in the middle of prose, which is the kind of bug that makes a feature feel broken
//!   rather than incomplete.
//! - **A reference the reader cannot open stays plain text** — no link that lands on a 404, and no
//!   existence oracle either.
//! - **Code and URLs are not prose.** `git log AWTD-9` in backticks is a command, and a path inside
//!   a URL is part of an address.
//!
//! Rendering costs no lookup: every link points at `/t/<IDENTIFIER>`, and that route resolves the
//! id, checks access and redirects. So a comment with forty references is still one request.

use serde::Serialize;

/// Project keys are short enough to type and long enough to disambiguate.
pub const MIN_PROJECT_KEY_LENGTH: usize = 2;
pub const MAX_PROJECT_KEY_LENGTH: usize = 5;

/// `AWTD-1007`, in pieces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedIdentifier {
    /// Always uppercase, whatever the input looked like.
    pub key: String,
    pub sequence: u64,
}

/// Read `AWTD-1007` — or `awtd-1007` — into its parts, or `None` when it is not an identifier.
///
/// Case-insensitive in, canonical uppercase out: one identifier must not resolve two different
/// ways. Surrounding whitespace is trimmed, because this is what a pasted id arrives as.
///
/// `None` covers everything that merely looks close: a key shorter than
/// [`MIN_PROJECT_KEY_LENGTH`] or longer than [`MAX_PROJECT_KEY_LENGTH`], a key starting with a
/// digit, sequence `0` (they start at 1), a trailing letter, a bare `#1007` — which is an autolink
/// rule rather than an identifier — and a UUID, which is the other thing a task id can be.
pub fn parse_identifier(value: &str) -> Option<ParsedIdentifier> {
    let trimmed = value.trim();
    let (key, digits) = trimmed.split_once('-')?;

    if !(MIN_PROJECT_KEY_LENGTH..=MAX_PROJECT_KEY_LENGTH).contains(&key.len()) {
        return None;
    }
    let mut characters = key.chars();
    if !characters.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !characters.all(|character| character.is_ascii_alphanumeric()) {
        return None;
    }

    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A sequence too long for u64 is not an identifier we minted, so it is not one we resolve.
    let sequence = digits.parse::<u64>().ok()?;
    if sequence < 1 {
        return None;
    }

    Some(ParsedIdentifier {
        key: key.to_ascii_uppercase(),
        sequence,
    })
}

/// `AWTD-1007` from its parts, canonical.
pub fn format_identifier(key: &str, sequence: u64) -> String {
    format!("{}-{sequence}", key.to_ascii_uppercase())
}

/// What the reader can see, which is what decides whether a reference becomes a link.
#[derive(Debug, Clone, Default)]
pub struct LinkContext {
    /// The project whose task or chat this text belongs to. `#N` means this project's key, and
    /// outside a project it is plain text — a number with a hash in front of it.
    pub project_key: Option<String>,
    /// The project keys the reader can see. Only these link.
    pub keys: Vec<String>,
    /// Individual ids known to be hidden from the reader.
    pub hidden: Vec<String>,
}

/// One reference found in a text, ready to be drawn as a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierLink {
    /// The text as written — `AWTD-12` or `#12`.
    pub matched: String,
    /// Canonical id, always `KEY-N`, which is what `#12` expands to.
    pub identifier: String,
    /// Where it goes: `/t/<IDENTIFIER>`.
    pub href: String,
    /// Byte offset of [`Self::matched`] in the text that was passed in — not in the masked copy
    /// this scans, so a caller can slice the original around it.
    pub index: usize,
}

/// Find every task reference in a text that the reader could actually follow.
///
/// Both forms: `KEY-N` anywhere, uppercase only — so a branch name like `awtd-1007-fix` stays
/// prose — and `#N` when [`LinkContext::project_key`] says which project we are inside.
///
/// Results are ordered by position, which is the order a renderer consumes them in.
pub fn find_links(text: &str, context: &LinkContext) -> Vec<IdentifierLink> {
    if text.is_empty() {
        return Vec::new();
    }
    let keys: Vec<String> = context
        .keys
        .iter()
        .map(|key| key.to_ascii_uppercase())
        .collect();
    if keys.is_empty() {
        return Vec::new();
    }
    let hidden: Vec<String> = context
        .hidden
        .iter()
        .map(|id| id.to_ascii_uppercase())
        .collect();

    let masked = mask(text);
    let mut links: Vec<IdentifierLink> = Vec::new();

    let known = |key: &str| keys.iter().any(|candidate| candidate == key);
    let visible = |identifier: &str| !hidden.iter().any(|candidate| candidate == identifier);

    let mut at = 0;
    while at < masked.len() {
        match full_form_at(&masked, at) {
            Some((key, sequence, end)) => {
                if known(&key) {
                    let identifier = format_identifier(&key, sequence);
                    if visible(&identifier) {
                        links.push(IdentifierLink {
                            matched: text[at..end].to_string(),
                            href: format!("/t/{identifier}"),
                            identifier,
                            index: at,
                        });
                    }
                }
                // Past the whole match even when it did not become a link, the way a global regex
                // advances: the `AWTD-1` inside `XAWTD-1` is not a second candidate.
                at = end;
            }
            None => at += 1,
        }
    }

    if let Some(project_key) = context
        .project_key
        .as_deref()
        .map(str::to_ascii_uppercase)
        .filter(|key| known(key))
    {
        let mut at = 0;
        while at < masked.len() {
            match short_form_at(&masked, at) {
                Some((sequence, end)) => {
                    let identifier = format_identifier(&project_key, sequence);
                    if visible(&identifier) {
                        links.push(IdentifierLink {
                            matched: text[at..end].to_string(),
                            href: format!("/t/{identifier}"),
                            identifier,
                            index: at,
                        });
                    }
                    at = end;
                }
                None => at += 1,
            }
        }
    }

    links.sort_by_key(|link| link.index);
    links
}

/// The context for text read inside `project_id`, from the projects this device knows.
///
/// The reader's own projects ARE the visible keys: the store holds what the account can see, so a
/// key that is not there belongs to a board this reader cannot open — and `UTF-8` belongs to no
/// board at all. `project_id` is what lets `#12` mean anything; outside a project it stays text.
///
/// [`LinkContext::hidden`] is left empty deliberately. A client cannot know that one particular task
/// is invisible to the reader without asking — which is exactly the lookup this design avoids — so
/// the key check is the guard here, and a stale reference resolves to "not found" at `/t/<id>`
/// rather than leaking anything.
pub fn context_for(projects: &[crate::model::Project], project_id: Option<&str>) -> LinkContext {
    LinkContext {
        project_key: project_id.and_then(|id| {
            projects
                .iter()
                .find(|project| project.id == id)
                .and_then(|project| project.key.clone())
        }),
        keys: projects
            .iter()
            .filter_map(|project| project.key.clone())
            .collect(),
        hidden: Vec::new(),
    }
}

/// Rewrite every reference through `render`, leaving the rest of the text alone.
///
/// Mirrors web's `replaceIdentifierLinks`, and is how a renderer turns ids into links without
/// knowing anything about the format.
pub fn replace_links(
    text: &str,
    context: &LinkContext,
    render: impl Fn(&IdentifierLink) -> String,
) -> String {
    let links = find_links(text, context);
    if links.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for link in &links {
        out.push_str(&text[cursor..link.index]);
        out.push_str(&render(link));
        cursor = link.index + link.matched.len();
    }
    out.push_str(&text[cursor..]);
    out
}

/// A `KEY-N` starting exactly at `at`: its key, its sequence, and where it ends.
///
/// The key is tried longest-first, which is what the regex's greedy repetition does — `AWTD2-44` is
/// key `AWTD2`, not `AWTD` followed by something strange.
fn full_form_at(masked: &[u8], at: usize) -> Option<(String, u64, usize)> {
    if !before_is_free(masked, at) {
        return None;
    }
    if !masked[at].is_ascii_uppercase() {
        return None;
    }

    for length in (MIN_PROJECT_KEY_LENGTH..=MAX_PROJECT_KEY_LENGTH).rev() {
        let hyphen = at + length;
        if hyphen >= masked.len() || masked[hyphen] != b'-' {
            continue;
        }
        if !masked[at + 1..hyphen]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            continue;
        }
        let (sequence, end) = digits_at(masked, hyphen + 1)?;
        // Not before a word character or another hyphen: `AWTD-3x` is not a reference.
        if end < masked.len() && (is_word(masked[end]) || masked[end] == b'-') {
            return None;
        }
        return Some((
            String::from_utf8_lossy(&masked[at..hyphen]).into_owned(),
            sequence,
            end,
        ));
    }
    None
}

/// A `#N` starting exactly at `at`: its sequence, and where it ends.
fn short_form_at(masked: &[u8], at: usize) -> Option<(u64, usize)> {
    if masked[at] != b'#' {
        return None;
    }
    // Not an HTML entity, not a fragment in a path, and not the second hash of a heading.
    if at > 0 {
        let before = masked[at - 1];
        if is_word(before) || before == b'&' || before == b'#' || before == b'/' {
            return None;
        }
    }
    let (sequence, end) = digits_at(masked, at + 1)?;
    if end < masked.len() && is_word(masked[end]) {
        return None;
    }
    Some((sequence, end))
}

/// The run of digits at `at` as a number, and where it ends. `None` when there are none, when the
/// run is longer than any sequence we could have minted, or when it is zero.
fn digits_at(masked: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut end = at;
    while end < masked.len() && masked[end].is_ascii_digit() {
        end += 1;
    }
    if end == at {
        return None;
    }
    let sequence: u64 = std::str::from_utf8(&masked[at..end]).ok()?.parse().ok()?;
    if sequence < 1 {
        return None;
    }
    Some((sequence, end))
}

/// Is the character before `at` one that a reference may follow?
///
/// Not a word character, a hyphen, a slash, a dot or a hash — which between them rule out
/// `XAWTD-1`, a path segment, a version number and a URL fragment.
fn before_is_free(masked: &[u8], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let before = masked[at - 1];
    !(is_word(before) || before == b'-' || before == b'/' || before == b'.' || before == b'#')
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Code and URLs, blanked to spaces so the offsets of everything else stay true.
///
/// Fenced blocks first, then inline spans, then bare URLs — web's order, and it matters: a URL
/// inside a code fence is masked once, as code. Blanking happens byte by byte, which is what keeps
/// an offset honest in a text that is not ASCII.
fn mask(text: &str) -> Vec<u8> {
    let mut masked = text.as_bytes().to_vec();
    blank_fences(&mut masked);
    blank_inline_code(&mut masked);
    blank_urls(&mut masked);
    masked
}

/// Every fenced region, non-greedy, including the fences themselves.
fn blank_fences(masked: &mut [u8]) {
    let fence = *b"```";
    let mut at = 0;
    while let Some(start) = find(masked, &fence, at) {
        let end = match find(masked, &fence, start + fence.len()) {
            Some(end) => end + fence.len(),
            // An unclosed fence masks the rest, where web's non-greedy pattern would simply not
            // match. The difference only shows on text that is already malformed, and blanking is
            // the safer of the two: an id inside an unterminated fence is still inside code.
            None => masked.len(),
        };
        blank(masked, start, end);
        at = end;
    }
}

/// A code span, which never crosses a line.
fn blank_inline_code(masked: &mut [u8]) {
    let mut at = 0;
    while at < masked.len() {
        if masked[at] != b'`' {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < masked.len() && masked[end] != b'`' && masked[end] != b'\n' {
            end += 1;
        }
        if end < masked.len() && masked[end] == b'`' {
            blank(masked, at, end + 1);
            at = end + 1;
        } else {
            at += 1;
        }
    }
}

/// A bare URL, up to the next whitespace.
fn blank_urls(masked: &mut [u8]) {
    for scheme in [b"https://".as_slice(), b"http://".as_slice()] {
        let mut at = 0;
        while let Some(start) = find(masked, scheme, at) {
            // A scheme glued to a word is not a URL starting here.
            if start > 0 && is_word(masked[start - 1]) {
                at = start + scheme.len();
                continue;
            }
            let mut end = start;
            while end < masked.len() && !masked[end].is_ascii_whitespace() {
                end += 1;
            }
            blank(masked, start, end);
            at = end;
        }
    }
}

fn blank(masked: &mut [u8], start: usize, end: usize) {
    for byte in &mut masked[start..end] {
        *byte = b' ';
    }
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() || needle.len() > haystack.len() - from {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(project_key: Option<&str>, keys: &[&str]) -> LinkContext {
        LinkContext {
            project_key: project_key.map(str::to_string),
            keys: keys.iter().map(|key| key.to_string()).collect(),
            hidden: Vec::new(),
        }
    }

    #[test]
    fn an_id_is_read_case_insensitively_and_answered_in_canonical_form() {
        let parsed = parse_identifier(" awtd-1007 ").expect("an identifier");
        assert_eq!(parsed.key, "AWTD");
        assert_eq!(parsed.sequence, 1007);
        assert_eq!(format_identifier(&parsed.key, parsed.sequence), "AWTD-1007");
    }

    /// The fixture's own rejections, restated so a failure names the reason rather than a case
    /// index.
    #[test]
    fn what_is_not_an_identifier() {
        for input in [
            "A-1",
            "ABCDEF-1",
            "AB-0",
            "2AB-1",
            "AB-",
            "AB-1a",
            "#1007",
            "",
            "3f2a0c1e-1111-2222-3333-444455556666",
        ] {
            assert!(
                parse_identifier(input).is_none(),
                "{input:?} should not parse"
            );
        }
    }

    /// A sequence no minter could have produced is not an identifier, rather than a panic.
    #[test]
    fn an_absurd_sequence_is_not_an_identifier() {
        assert!(parse_identifier("AB-99999999999999999999999").is_none());
    }

    #[test]
    fn a_reference_the_reader_cannot_open_stays_text() {
        let mut ctx = context(None, &["AWTD"]);
        ctx.hidden = vec!["AWTD-3".to_string()];
        let links = find_links("See AWTD-3 and AWTD-4.", &ctx);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].identifier, "AWTD-4");
    }

    /// The offsets must index the ORIGINAL text, not the masked copy, and survive text that is not
    /// ASCII — the masking works in bytes precisely so this holds.
    #[test]
    fn offsets_point_into_the_original_text_even_after_a_wide_character() {
        let text = "Größe — siehe AWTD-12 dazu";
        let links = find_links(text, &context(None, &["AWTD"]));
        assert_eq!(links.len(), 1);
        assert_eq!(
            &text[links[0].index..links[0].index + links[0].matched.len()],
            "AWTD-12"
        );
    }

    #[test]
    fn a_short_form_needs_a_project_and_a_full_one_does_not() {
        assert!(find_links("Dup of #12.", &context(None, &["AWTD"])).is_empty());
        let inside = find_links("Dup of #12.", &context(Some("AWTD"), &["AWTD"]));
        assert_eq!(inside.len(), 1);
        assert_eq!(inside[0].identifier, "AWTD-12");
        assert_eq!(inside[0].matched, "#12");
    }

    /// A project the reader cannot see does not lend its key to the short form either.
    #[test]
    fn a_short_form_in_an_invisible_project_stays_text() {
        assert!(find_links("Dup of #12.", &context(Some("ZZZZ"), &["AWTD"])).is_empty());
    }

    #[test]
    fn code_and_urls_are_not_prose() {
        let links = find_links(
            "Run `git log AWTD-9` then read AWTD-10, not https://x.test/AWTD-11",
            &context(None, &["AWTD"]),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].identifier, "AWTD-10");
    }

    /// An unterminated fence keeps its contents out of prose rather than letting the rest of the
    /// message autolink through what the writer clearly meant as code.
    #[test]
    fn an_unclosed_fence_masks_what_follows_it() {
        assert!(find_links("```\nAWTD-1", &context(None, &["AWTD"])).is_empty());
    }

    /// What a renderer does with the spans: the text between them must survive untouched, and two
    /// references in one sentence must not shift each other's offsets.
    #[test]
    fn replacing_leaves_everything_between_the_references_alone() {
        let rewritten = replace_links(
            "Blocked by AWTD-1007 and AWTD-8; ask AITD-2.",
            &context(None, &["AWTD"]),
            |link| format!("[{}]({})", link.matched, link.href),
        );
        assert_eq!(
            rewritten,
            "Blocked by [AWTD-1007](/t/AWTD-1007) and [AWTD-8](/t/AWTD-8); ask AITD-2."
        );
    }

    #[test]
    fn replacing_nothing_returns_the_text_as_it_came() {
        let text = "Nothing to see here.";
        assert_eq!(
            replace_links(text, &context(None, &["AWTD"]), |_| String::new()),
            text
        );
    }

    fn project(id: &str, key: Option<&str>) -> crate::model::Project {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": id,
            "key": key,
        }))
        .expect("a project")
    }

    /// The reader's own boards are the visible keys, and the board they are reading inside is the
    /// one whose key `#N` means.
    #[test]
    fn a_context_is_built_from_the_boards_this_device_knows() {
        let projects = [
            project("p1", Some("AWTD")),
            project("p2", Some("AITD")),
            project("p3", None),
        ];

        let inside = context_for(&projects, Some("p2"));
        assert_eq!(inside.project_key.as_deref(), Some("AITD"));
        assert_eq!(inside.keys, vec!["AWTD".to_string(), "AITD".to_string()]);
        assert!(inside.hidden.is_empty());

        let outside = context_for(&projects, None);
        assert_eq!(outside.project_key, None);

        // A board from before identifiers existed lends no key to anything.
        assert_eq!(context_for(&projects, Some("p3")).project_key, None);
    }
}
