//! GHD's issue markdown filters (`app/src/lib/markdown-filters/
//! issue-mention-filter.ts`, `issue-link-filter.ts`, `resolve-owner-repo.ts`)
//! over [`RichText`] runs: text references to issues, pull requests and
//! discussions become links, and pasted links to them read `#123`.
//!
//! GHD runs them in `SandboxedMarkdown` when it has the content's repository
//! (the pull request quick view and notifications); Corvene runs them on
//! every GitHub body it shows (issues, comments, releases, pull requests,
//! reviews). A click on such a link offers the in-app view
//! (`355-github-links-in-app`, `corvene_ui::link_menu`).
//!
//! Deviation: a pasted link reads `#123` without GHD's trailing space.

use std::ops::Range;

use corvene_models::GitHubRepository;

use crate::markdown::{InlineStyle, RichText};
use crate::state::AppState;

/// The repository references are relative to: `#1` is its issue 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceRepository {
    /// `getHTMLURL(endpoint)`: `https://github.com`.
    pub web_base: String,
    pub owner: String,
    pub name: String,
}

impl ReferenceRepository {
    pub fn of(gh: &GitHubRepository) -> Self {
        Self {
            web_base: corvene_github::Endpoint::from_api_base(&gh.endpoint).web_base,
            owner: gh.owner.clone(),
            name: gh.name.clone(),
        }
    }
}

impl AppState {
    /// The GitHub repository (an added one or its parent) whose page `url`
    /// is: Markdown with that base link resolves its references there.
    pub fn reference_repository_for(&self, url: &str) -> Option<ReferenceRepository> {
        let url = url.to_ascii_lowercase();
        self.repositories
            .iter()
            .filter_map(|r| r.github.as_ref())
            .flat_map(|gh| std::iter::once(gh).chain(gh.parent.as_deref()))
            .find(|gh| {
                let page = gh.html_url.trim_end_matches('/').to_ascii_lowercase();
                url.strip_prefix(&page)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '#', '?']))
                    && self.web_host_kind(&gh.endpoint) == corvene_models::HostKind::GitHub
            })
            .map(ReferenceRepository::of)
    }
}

/// JavaScript's `\w`.
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `text` with GHD's `IssueMentionFilter` and `IssueLinkFilter` applied;
/// `None` when nothing changed. Code and links keep their text (GHD skips
/// text in `code`, `pre` and `a`); each unlinked run is matched on its own,
/// as GHD matches each text node.
pub fn link_issue_references(text: &RichText, repo: &ReferenceRepository) -> Option<RichText> {
    // the runs: (range, style, link)
    let mut runs: Vec<(Range<usize>, InlineStyle, Option<&str>)> = Vec::new();
    let mut at = 0;
    for span in &text.spans {
        if span.range.start > at {
            runs.push((at..span.range.start, InlineStyle::default(), None));
        }
        runs.push((span.range.clone(), span.style, span.link.as_deref()));
        at = span.range.end;
    }
    if at < text.text.len() {
        runs.push((at..text.text.len(), InlineStyle::default(), None));
    }

    let mut out = RichText::default();
    // (old range, new length) of every replaced piece
    let mut edits: Vec<(Range<usize>, usize)> = Vec::new();
    let mut changed = false;
    for (range, style, link) in runs {
        let s = &text.text[range.clone()];
        match link {
            Some(href) if !style.code && s == href => match issue_link_text(href, repo) {
                Some(short) => {
                    edits.push((range.clone(), short.len()));
                    out.push(&short, style, Some(href));
                    changed = true;
                }
                None => out.push(s, style, Some(href)),
            },
            Some(href) => out.push(s, style, Some(href)),
            None if style.code => out.push(s, style, None),
            None => {
                let mut last = 0;
                for (m, display, href) in issue_mentions(s, repo) {
                    out.push(&s[last..m.start], style, None);
                    out.push(&display, style, Some(&href));
                    if display.len() != m.len() {
                        edits.push((range.start + m.start..range.start + m.end, display.len()));
                    }
                    last = m.end;
                    changed = true;
                }
                out.push(&s[last..], style, None);
            }
        }
    }
    if !changed {
        return None;
    }
    let map = |old: usize| {
        let mut delta: isize = 0;
        for (range, len) in &edits {
            if old >= range.end {
                delta += *len as isize - range.len() as isize;
            } else if old > range.start {
                let into = (old - range.start).min(*len);
                return (range.start as isize + delta) as usize + into;
            }
        }
        (old as isize + delta) as usize
    };
    out.images = text
        .images
        .iter()
        .cloned()
        .map(|mut i| {
            i.at = map(i.at);
            i
        })
        .collect();
    out.maths = text
        .maths
        .iter()
        .cloned()
        .map(|mut m| {
            m.at = map(m.at);
            m
        })
        .collect();
    Some(out)
}

/// `IssueMentionMarker`, matched without regard to case.
const MARKERS: [&str; 5] = ["#", "gh-", "/issues/", "/pull/", "/discussions/"];

/// The marker and number at `q`: (end of the marker, end of the number).
fn marker_at(b: &[u8], q: usize) -> Option<(usize, usize)> {
    let marker_end = MARKERS.iter().find_map(|m| {
        let end = q + m.len();
        (end <= b.len() && b[q..end].eq_ignore_ascii_case(m.as_bytes())).then_some(end)
    })?;
    let digits = b[marker_end..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    let end = marker_end + digits;
    // `\d+\b`
    (digits > 0 && b.get(end).is_none_or(|&c| !is_word(c))).then_some((marker_end, end))
}

/// `IssueReference` at `p`: (end of the owner / repository part, end of the
/// marker, end of the match), trying the alternatives in the order the
/// regular expression's backtracking does.
fn reference_at(b: &[u8], p: usize) -> Option<(usize, usize, usize)> {
    // `\w+(?:-\w+)*`: every end after a word character
    let mut owner_ends = Vec::new();
    let mut j = p;
    while j < b.len() && is_word(b[j]) {
        j += 1;
        owner_ends.push(j);
        if j < b.len() && b[j] == b'-' && b.get(j + 1).is_some_and(|&c| is_word(c)) {
            j += 1;
        }
    }
    for &k in owner_ends.iter().rev() {
        // `(?:\/[.\w-]+)?`
        if b.get(k) == Some(&b'/') {
            let run = b[k + 1..]
                .iter()
                .take_while(|&&c| is_word(c) || c == b'.' || c == b'-')
                .count();
            for m in (k + 2..=k + 1 + run).rev() {
                if let Some((marker_end, end)) = marker_at(b, m) {
                    return Some((m, marker_end, end));
                }
            }
        }
        if let Some((marker_end, end)) = marker_at(b, k) {
            return Some((k, marker_end, end));
        }
    }
    marker_at(b, p).map(|(marker_end, end)| (p, marker_end, end))
}

/// GHD `IssueMentionFilter.filter` over one text run: each reference's
/// range, link text and link.
fn issue_mentions(s: &str, repo: &ReferenceRepository) -> Vec<(Range<usize>, String, String)> {
    let b = s.as_bytes();
    let mut found = Vec::new();
    let mut p = 0;
    while p < b.len() {
        // `(?<=^|\W)`
        if p > 0 && is_word(b[p - 1]) {
            p += 1;
            continue;
        }
        let Some((owner_end, marker_end, end)) = reference_at(b, p) else {
            p += 1;
            continue;
        };
        let owner_or_owner_repo = (owner_end > p).then(|| &s[p..owner_end]);
        let reference = &s[owner_end..end];
        let number = &s[marker_end..end];
        if let Some((display, owner, name)) = resolve(owner_or_owner_repo, reference, repo) {
            let href = format!("{}/{owner}/{name}/issues/{number}", repo.web_base);
            found.push((p..end, display, href));
        }
        p = end;
    }
    found
}

/// `createLinkElement` with `resolveOwnerRepo`: the link text and the
/// repository, or `None` for a lone name that is not the owner's.
fn resolve<'a>(
    owner_or_owner_repo: Option<&'a str>,
    reference: &str,
    repo: &'a ReferenceRepository,
) -> Option<(String, &'a str, &'a str)> {
    let here = (
        reference.to_string(),
        repo.owner.as_str(),
        repo.name.as_str(),
    );
    let Some(written) = owner_or_owner_repo else {
        return Some(here);
    };
    match written.split_once('/') {
        None if written == repo.owner => Some(here),
        None => None,
        Some((owner, name)) if owner != repo.owner || name != repo.name => {
            Some((format!("{written}{reference}"), owner, name))
        }
        Some(_) => Some(here),
    }
}

/// GHD `IssueLinkFilter`: the text a pasted link to an issue, pull request
/// or discussion of a repository on `repo`'s host reads, `#123` (with
/// `(comment)` and the like for an anchor).
fn issue_link_text(href: &str, repo: &ReferenceRepository) -> Option<String> {
    if !["issue", "pull", "discussion"]
        .iter()
        .any(|k| href.contains(k))
    {
        return None;
    }
    // a pull request's other tabs
    let tab = ["/files", "/commits", "/conflicts", "/checks"]
        .iter()
        .any(|tab| {
            href.match_indices(tab)
                .any(|(i, _)| i > 0 && href.as_bytes()[i - 1].is_ascii_digit())
        });
    if tab {
        return None;
    }
    let rest = href
        .strip_prefix(repo.web_base.as_str())?
        .strip_prefix('/')?;
    let mut parts = rest.splitn(4, '/');
    let owner = parts.next()?;
    let name = parts.next()?;
    let kind = parts.next()?;
    let tail = parts.next()?;
    let owner_ok = !owner.is_empty()
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && !owner.contains("--")
        && owner.bytes().all(|c| is_word(c) || c == b'-');
    let name_ok = !name.is_empty() && name.bytes().all(|c| is_word(c) || c == b'.' || c == b'-');
    if !owner_ok || !name_ok || !["issues", "pull", "discussions"].contains(&kind) {
        return None;
    }
    let t = tail.as_bytes();
    let digits = t.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let number = &tail[..digits];
    let anchor_len = if t.get(digits) == Some(&b'#') {
        t[digits + 1..]
            .iter()
            .take_while(|&&c| is_word(c) || c == b'-')
            .count()
    } else {
        0
    };
    let anchor = (anchor_len > 0).then(|| &tail[digits + 1..digits + 1 + anchor_len]);
    let end = if anchor.is_some() {
        digits + 1 + anchor_len
    } else {
        digits
    };
    // `\b`
    let before = t[end - 1];
    if t.get(end).is_some_and(|&c| is_word(c) == is_word(before)) {
        return None;
    }
    let mut text = format!("#{number}");
    if let Some(anchor) = anchor {
        let description = if anchor.contains("discussion-diff-") {
            "(diff)"
        } else if anchor.contains("commits-pushed-") {
            "(commits)"
        } else if anchor.contains("ref-") {
            "(reference)"
        } else if anchor.contains("pullrequestreview") {
            "(review)"
        } else {
            "(comment)"
        };
        text.push(' ');
        text.push_str(description);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{Block, parse};

    fn repo() -> ReferenceRepository {
        ReferenceRepository {
            web_base: "https://github.com".into(),
            owner: "tidy-dev".into(),
            name: "foo".into(),
        }
    }

    fn linked(md: &str) -> RichText {
        let blocks = parse(md);
        let Block::Paragraph(text) = &blocks[0] else {
            panic!("not a paragraph: {blocks:?}");
        };
        link_issue_references(text, &repo()).unwrap_or_else(|| text.clone())
    }

    fn links(text: &RichText) -> Vec<(&str, &str)> {
        text.links()
            .map(|(r, href)| (&text.text[r.clone()], href))
            .collect()
    }

    #[test]
    fn mentions() {
        let t = linked("Issue #1234 is the same thing");
        assert_eq!(t.text, "Issue #1234 is the same thing");
        assert_eq!(
            links(&t),
            [("#1234", "https://github.com/tidy-dev/foo/issues/1234")]
        );
        for (md, text) in [
            ("gh-12.", "gh-12"),
            ("GH-12", "GH-12"),
            ("see /pull/7", "/pull/7"),
            ("x /discussions/9", "/discussions/9"),
        ] {
            let t = linked(md);
            assert_eq!(links(&t)[0].0, text, "{md}");
        }
    }

    #[test]
    fn owners() {
        // redundant owners are trimmed
        let t = linked("tidy-dev/foo#1 and tidy-dev#2");
        assert_eq!(t.text, "#1 and #2");
        assert_eq!(links(&t)[1].1, "https://github.com/tidy-dev/foo/issues/2");
        // another repository stays as written
        let t = linked("desktop/dugite#1 desktop/dugite/issues/1234");
        assert_eq!(
            links(&t),
            [
                (
                    "desktop/dugite#1",
                    "https://github.com/desktop/dugite/issues/1"
                ),
                (
                    "desktop/dugite/issues/1234",
                    "https://github.com/desktop/dugite/issues/1234"
                ),
            ]
        );
        // a lone repository name is no reference
        assert!(links(&linked("foo#1")).is_empty());
    }

    #[test]
    fn non_matches() {
        for md in [
            "a#1",
            "x #12a",
            "x #",
            "x #x",
            "`#1`",
            "[#1](https://example.com)",
        ] {
            let t = linked(md);
            assert!(
                t.links().all(|(_, href)| !href.contains("/issues/")),
                "{md}: {t:?}"
            );
        }
    }

    #[test]
    fn styled_runs() {
        let t = linked("**see #3** now");
        assert_eq!(
            links(&t),
            [("#3", "https://github.com/tidy-dev/foo/issues/3")]
        );
        assert!(t.spans.iter().all(|s| s.style.bold || s.range.start >= 9));
    }

    #[test]
    fn pasted_links() {
        let t = linked("Fixed by https://github.com/tidy-dev/foo/pull/42 today");
        assert_eq!(t.text, "Fixed by #42 today");
        assert_eq!(
            links(&t),
            [("#42", "https://github.com/tidy-dev/foo/pull/42")]
        );
        let t = linked("https://github.com/o/r/issues/5#issuecomment-1");
        assert_eq!(t.text, "#5 (comment)");
        let t = linked("https://github.com/o/r/pull/5#pullrequestreview-9");
        assert_eq!(t.text, "#5 (review)");
        // other tabs and hosts stay
        for url in [
            "https://github.com/o/r/pull/5/files",
            "https://example.com/o/r/issues/5",
        ] {
            assert_eq!(linked(url).text, url);
        }
        // a link with its own text stays
        assert_eq!(
            linked("[the fix](https://github.com/o/r/pull/5)").text,
            "the fix"
        );
    }

    #[test]
    fn images_shift() {
        let t = linked("https://github.com/o/r/issues/5 ![x](a.png)");
        assert_eq!(t.text, "#5 ");
        assert_eq!(t.images[0].at, 3);
    }
}
