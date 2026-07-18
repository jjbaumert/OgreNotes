//! Mermaid `gitGraph`: parser + lane-based SVG renderer.
//!
//! Commits advance a global left-to-right sequence; each branch is a
//! horizontal lane. `commit` / `branch` / `checkout`(`switch`) / `merge`
//! are supported, with `id:` / `tag:` / `type:` options. Rendering is
//! always left-to-right (the most common orientation).

use crate::{escape_xml, ParseError};
use std::collections::HashMap;

const MAX_COMMITS: usize = 300;
const MAX_BRANCHES: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitType {
    Normal,
    Reverse,
    Highlight,
    CherryPick,
}

#[derive(Debug, Clone)]
pub(crate) struct Commit {
    pub id: Option<String>,
    pub lane: usize,
    pub seq: usize,
    pub parents: Vec<usize>,
    pub tag: Option<String>,
    pub ctype: CommitType,
}

#[derive(Debug, Clone)]
pub(crate) struct GitGraph {
    pub commits: Vec<Commit>,
    pub branches: Vec<String>, // lane index -> branch name
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

/// Pulls `key: value` (value may be `"quoted"` or a bare token) out of an
/// option string, returning the value. The key must appear at a word
/// boundary (start or after whitespace) and be followed by `:`, so a key
/// substring *inside* a quoted value (e.g. `tag: "id: 5"`) can't match.
/// Case-sensitive keys.
fn option(opts: &str, key: &str) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = opts[from..].find(key) {
        let at = from + rel;
        from = at + key.len();
        if at != 0 && !opts[..at].ends_with(char::is_whitespace) {
            continue; // not a word boundary — inside another token/value
        }
        let after = opts[at + key.len()..].trim_start();
        let Some(value) = after.strip_prefix(':') else {
            continue; // `key` not used as an option key here
        };
        let value = value.trim_start();
        return Some(if let Some(rest) = value.strip_prefix('"') {
            rest.split('"').next().unwrap_or("").to_string()
        } else {
            value.split_whitespace().next().unwrap_or("").to_string()
        });
    }
    None
}

pub(crate) fn parse(source: &str) -> Result<GitGraph, ParseError> {
    let mut commits: Vec<Commit> = Vec::new();
    let mut branches: Vec<String> = vec!["main".to_string()];
    let mut lane_of: HashMap<String, usize> = HashMap::from([("main".to_string(), 0)]);
    let mut tip: HashMap<String, Option<usize>> = HashMap::from([("main".to_string(), None)]);
    let mut current = "main".to_string();
    let mut seq = 0usize;
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            // `gitGraph`, `gitGraph:`, `gitGraph LR:`, `gitGraph TB:`.
            let head = line.split_whitespace().next().unwrap_or("");
            if head.trim_end_matches(':') != "gitGraph" {
                return Err(err("git graph must start with `gitGraph`", line_no));
            }
            seen_header = true;
            continue;
        }

        let (kw, args) = match line.split_once(char::is_whitespace) {
            Some((k, a)) => (k, a.trim()),
            None => (line, ""),
        };
        match kw {
            "commit" => {
                let parents: Vec<usize> = tip[&current].into_iter().collect();
                let ctype = match option(args, "type").as_deref() {
                    Some("REVERSE") => CommitType::Reverse,
                    Some("HIGHLIGHT") => CommitType::Highlight,
                    _ => CommitType::Normal,
                };
                if commits.len() >= MAX_COMMITS {
                    return Err(err(format!("git graph too large: more than {MAX_COMMITS} commits"), line_no));
                }
                let ci = commits.len();
                commits.push(Commit {
                    id: option(args, "id"),
                    lane: lane_of[&current],
                    seq,
                    parents,
                    tag: option(args, "tag"),
                    ctype,
                });
                seq += 1;
                tip.insert(current.clone(), Some(ci));
            }
            "branch" => {
                let name = args.split_whitespace().next().unwrap_or("").to_string();
                if name.is_empty() {
                    return Err(err("branch needs a name", line_no));
                }
                if lane_of.contains_key(&name) {
                    return Err(err(format!("branch `{name}` already exists"), line_no));
                }
                if branches.len() >= MAX_BRANCHES {
                    return Err(err(format!("git graph too large: more than {MAX_BRANCHES} branches"), line_no));
                }
                let lane = branches.len();
                branches.push(name.clone());
                lane_of.insert(name.clone(), lane);
                // Branch from the current tip, then check the new branch out.
                tip.insert(name.clone(), tip[&current]);
                current = name;
            }
            "checkout" | "switch" => {
                let name = args.split_whitespace().next().unwrap_or("");
                if !lane_of.contains_key(name) {
                    return Err(err(format!("checkout of unknown branch `{name}`"), line_no));
                }
                current = name.to_string();
            }
            "merge" => {
                let name = args.split_whitespace().next().unwrap_or("");
                let Some(&_lane) = lane_of.get(name) else {
                    return Err(err(format!("merge of unknown branch `{name}`"), line_no));
                };
                if name == current {
                    return Err(err("cannot merge a branch into itself", line_no));
                }
                let mut parents: Vec<usize> = Vec::new();
                parents.extend(tip[&current]);
                parents.extend(tip[name]);
                if commits.len() >= MAX_COMMITS {
                    return Err(err(format!("git graph too large: more than {MAX_COMMITS} commits"), line_no));
                }
                let ctype = match option(args, "type").as_deref() {
                    Some("REVERSE") => CommitType::Reverse,
                    Some("HIGHLIGHT") => CommitType::Highlight,
                    _ => CommitType::Normal,
                };
                let ci = commits.len();
                commits.push(Commit {
                    id: option(args, "id"),
                    lane: lane_of[&current],
                    seq,
                    parents,
                    tag: option(args, "tag"),
                    ctype,
                });
                seq += 1;
                tip.insert(current.clone(), Some(ci));
            }
            "cherry-pick" => {
                let pick = option(args, "id")
                    .ok_or_else(|| err("`cherry-pick` needs an `id:`", line_no))?;
                if !commits.iter().any(|c| c.id.as_deref() == Some(pick.as_str())) {
                    return Err(err(
                        format!("cherry-pick references unknown commit `{pick}`"),
                        line_no,
                    ));
                }
                if commits.len() >= MAX_COMMITS {
                    return Err(err(
                        format!("git graph too large: more than {MAX_COMMITS} commits"),
                        line_no,
                    ));
                }
                let parents: Vec<usize> = tip[&current].into_iter().collect();
                let ci = commits.len();
                commits.push(Commit {
                    id: Some(pick), // label the pick with the source commit id
                    lane: lane_of[&current],
                    seq,
                    parents,
                    tag: option(args, "tag"),
                    ctype: CommitType::CherryPick,
                });
                seq += 1;
                tip.insert(current.clone(), Some(ci));
            }
            other => {
                return Err(err(format!("unsupported git graph statement {other:?}"), line_no));
            }
        }
    }

    if !seen_header {
        return Err(ParseError { message: "git graph must start with `gitGraph`".into(), line: None });
    }
    if commits.is_empty() {
        return Err(ParseError { message: "git graph has no commits".into(), line: None });
    }
    Ok(GitGraph { commits, branches })
}

const LABEL_W: f64 = 74.0;
const COMMIT_GAP: f64 = 50.0; // D10: mermaid commit x-spacing
const LANE_GAP: f64 = 90.0; // D3: 50 + 40 (rotateCommitLabel on) for LR
const TOP: f64 = 30.0;
const DOT_R: f64 = 10.0; // D2: normal commit radius
const MERGE_R_OUTER: f64 = 9.0; // D2
const MERGE_R_INNER: f64 = 6.0;
const ARROW_W: f64 = 8.0; // D1: commit arrow stroke width
const CORNER: f64 = 20.0; // D8: branch/merge path corner radius

/// Mermaid default-theme `git0`..`git7`: `darken(adjust(primaryColor, hueShift),
/// 25)` (light mode). `#ECECFF` = hsl(240,100%,96.27%), `#ffffde` =
/// hsl(60,100%,93.53%); darkening 25 gives the lightnesses below. SVG accepts
/// `hsl(...)` fills, so the resolved values are used verbatim.
const GIT_COLORS: &[&str] = &[
    "hsl(240, 100%, 71.27%)", // git0 (main)  #6C6CFF
    "hsl(60, 100%, 68.53%)",  // git1 (develop) #FFFF5E
    "hsl(80, 100%, 71.27%)",  // git2 = tertiary (h-160)
    "hsl(210, 100%, 71.27%)", // git3 (h-30)
    "hsl(180, 100%, 71.27%)", // git4 (h-60)
    "hsl(150, 100%, 71.27%)", // git5 (h-90)
    "hsl(300, 100%, 71.27%)", // git6 (h+60)
    "hsl(0, 100%, 71.27%)",   // git7 (h+120)
];
/// `gitBranchLabel0`..`7`: `invert(#333)=#ccc` for lanes 0 and 3, else `#333`.
const GIT_LABEL_COLORS: &[&str] =
    &["#ccc", "#333", "#333", "#ccc", "#333", "#333", "#333", "#333"];
/// Dashed per-branch rule color (mermaid `lineColor`).
const LINE_COLOR: &str = "#999999";

fn git_color(lane: usize) -> &'static str {
    GIT_COLORS[lane % GIT_COLORS.len()]
}
fn git_label_color(lane: usize) -> &'static str {
    GIT_LABEL_COLORS[lane % GIT_LABEL_COLORS.len()]
}

/// Deterministic 7-hex-char hash for a commit's generated id (the crate forbids
/// nondeterministic output; mermaid uses a random 7-char suffix).
fn hash7(seq: usize) -> String {
    let mut h = (seq as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    format!("{:07x}", h & 0xFFF_FFFF)
}

/// D4: a generated commit id is `seq-hash`; an explicit `id:` wins.
fn commit_id(c: &Commit) -> String {
    c.id.clone().unwrap_or_else(|| format!("{}-{}", c.seq, hash7(c.seq)))
}

/// D8: an orthogonal rounded path between two commits on different lanes.
/// `fork` (child deeper): drop vertically at the parent x, then run to the
/// child. Otherwise (merge, child shallower): run horizontally at the parent y,
/// then rise vertically at the child x. Corner radius `CORNER`.
fn ortho_path(px: f64, py: f64, cx: f64, cy: f64, fork: bool) -> String {
    let r = CORNER;
    if fork {
        let vsign = (cy - py).signum();
        // vertical at px down to the corner, quarter turn, horizontal to cx.
        format!(
            "M {px:.1} {py:.1} L {px:.1} {:.1} Q {px:.1} {cy:.1} {:.1} {cy:.1} L {cx:.1} {cy:.1}",
            cy - r * vsign,
            px + r,
        )
    } else {
        let vsign = (cy - py).signum();
        // horizontal at py to the corner, quarter turn, vertical up/down to cy.
        format!(
            "M {px:.1} {py:.1} L {:.1} {py:.1} Q {cx:.1} {py:.1} {cx:.1} {:.1} L {cx:.1} {cy:.1}",
            cx - r,
            py + r * vsign,
        )
    }
}

pub(crate) fn render_svg(g: &GitGraph) -> String {
    let max_seq = g.commits.iter().map(|c| c.seq).max().unwrap_or(0);
    let n_lanes = g.branches.len();
    let x_of = |seq: usize| LABEL_W + 20.0 + seq as f64 * COMMIT_GAP;
    let y_of = |lane: usize| TOP + lane as f64 * LANE_GAP;
    let w = x_of(max_seq) + 60.0;
    // Bottom margin must fit the rotated id labels, which are END-anchored
    // just below their dot and trail down-left at 45° — the vertical drop is
    // label_width/√2 (plus glyph extent), measured at the 11px label size.
    let label_drop = g
        .commits
        .iter()
        .map(|c| crate::measure::text_size(&commit_id(c)).0 * (11.0 / crate::measure::FONT_PX))
        .fold(0.0, f64::max)
        / std::f64::consts::SQRT_2;
    let h = y_of(n_lanes.saturating_sub(1)) + DOT_R + 10.0 + label_drop + 14.0;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:12px">"#
    );

    // D7: thin dashed per-branch rule spanning the diagram width, UNDER the
    // thick arrows (on a busy lane most of it is hidden behind the 8px stroke).
    for lane in 0..n_lanes {
        let ly = y_of(lane);
        svg.push_str(&format!(
            r#"<line x1="0" y1="{ly:.1}" x2="{w:.0}" y2="{ly:.1}" stroke="{LINE_COLOR}" stroke-width="1" stroke-dasharray="2"/>"#
        ));
    }

    // Arrows (under the dots): parent -> commit. Same-lane edges are a straight
    // 8px rule; cross-lane edges route orthogonally with rounded corners.
    for c in &g.commits {
        let (cx, cy) = (x_of(c.seq), y_of(c.lane));
        let is_merge = c.parents.len() >= 2;
        for (k, &p) in c.parents.iter().enumerate() {
            let pc = &g.commits[p];
            let (px, py) = (x_of(pc.seq), y_of(pc.lane));
            // D8 color rule: the child's branch, except a merge's non-first
            // parent edge takes the merged (parent) branch's color.
            let color = if is_merge && k != 0 { git_color(pc.lane) } else { git_color(c.lane) };
            let d = if pc.lane == c.lane {
                format!("M {px:.1} {py:.1} L {cx:.1} {cy:.1}")
            } else {
                ortho_path(px, py, cx, cy, c.lane > pc.lane)
            };
            svg.push_str(&format!(
                r#"<path d="{d}" stroke="{color}" stroke-width="{ARROW_W}" stroke-linecap="round" fill="none"/>"#
            ));
        }
    }

    // D6: branch labels — a rounded rect filled with the branch color, text in
    // the matching gitBranchLabel color, right-aligned in the left gutter.
    for (lane, name) in g.branches.iter().enumerate() {
        let (tw, _) = crate::measure::text_size(name);
        let by = y_of(lane);
        let rw = tw + 12.0;
        let rx = LABEL_W - rw;
        svg.push_str(&format!(
            r#"<rect x="{rx:.1}" y="{:.1}" width="{rw:.1}" height="20" rx="4" ry="4" fill="{}"/>"#,
            by - 10.0,
            git_color(lane),
        ));
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="{}" font-weight="600">{}</text>"#,
            rx + rw / 2.0,
            by + 4.0,
            git_label_color(lane),
            escape_xml(name),
        ));
    }

    // Commit dots + tag/id labels.
    for c in &g.commits {
        let (cx, cy) = (x_of(c.seq), y_of(c.lane));
        let color = git_color(c.lane);
        let is_merge = c.parents.len() >= 2;
        match c.ctype {
            _ if is_merge => {
                // D2: merge = outer r9 + inner r6 (the ring), branch-colored.
                svg.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{MERGE_R_OUTER}" fill="{color}"/><circle cx="{cx:.1}" cy="{cy:.1}" r="{MERGE_R_INNER}" fill="var(--surface, #fff)"/>"#
                ));
            }
            CommitType::Highlight => {
                svg.push_str(&format!(
                    r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{color}" stroke="currentColor" stroke-width="2"/>"#,
                    cx - DOT_R, cy - DOT_R, DOT_R * 2.0, DOT_R * 2.0,
                ));
            }
            CommitType::Reverse => {
                svg.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{DOT_R}" fill="var(--surface, #fff)" stroke="{color}" stroke-width="2"/>"#
                ));
            }
            CommitType::Normal => {
                svg.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{DOT_R}" fill="{color}"/>"#
                ));
            }
            CommitType::CherryPick => {
                let a = DOT_R * 0.55;
                svg.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{DOT_R}" fill="{color}"/><path d="M {:.1} {:.1} L {:.1} {:.1} M {:.1} {:.1} L {:.1} {:.1}" stroke="var(--surface, #fff)" stroke-width="1.5"/>"#,
                    cx - a, cy, cx + a, cy, cx, cy - a, cx, cy + a,
                ));
            }
        }
        if let Some(tag) = &c.tag {
            svg.push_str(&format!(
                r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" style="font-weight:600">{}</text>"#,
                cy - DOT_R - 6.0,
                escape_xml(tag),
            ));
        }
        // D5: merge commits render no id label (unless an explicit id was given).
        if is_merge && c.id.is_none() {
            continue;
        }
        // D3: commit id labels are rotated -45deg about their anchor point.
        // Upstream anchors the END of the text just below the dot so the
        // label trails away down-left; a start-anchored label at the same
        // point would instead run up-right *through* the dot and lane line.
        let ly = cy + DOT_R + 10.0;
        svg.push_str(&format!(
            r#"<text x="{cx:.1}" y="{ly:.1}" text-anchor="end" transform="rotate(-45 {cx:.1} {ly:.1})" fill="currentColor" style="font-size:11px">{}</text>"#,
            escape_xml(&commit_id(c)),
        ));
    }

    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> GitGraph {
        parse(src).expect("parse ok")
    }





    #[test]
    fn header_required() {
        assert!(parse("commit").is_err());
        assert!(parse("gitGraph\ncommit").is_ok());
        assert!(parse("gitGraph:\ncommit").is_ok());
        assert!(parse("gitGraph LR:\ncommit").is_ok());
    }

    #[test]
    fn commits_chain_on_main() {
        let g = p("gitGraph\ncommit\ncommit\ncommit");
        assert_eq!(g.commits.len(), 3);
        assert_eq!(g.branches, vec!["main"]);
        assert!(g.commits[0].parents.is_empty());
        assert_eq!(g.commits[1].parents, vec![0]);
        assert_eq!(g.commits[2].parents, vec![1]);
        // seqs advance left to right.
        assert_eq!((g.commits[0].seq, g.commits[2].seq), (0, 2));
    }

    #[test]
    fn commit_ids_seq_prefixed_and_merge_unlabeled_and_ringed() {
        // D4: generated ids are `seq-hash`; both plain commits are labeled.
        let plain = render_svg(&p("gitGraph\ncommit\ncommit"));
        assert!(plain.contains(">0-") && plain.contains(">1-"), "seq-prefixed ids: {plain}");
        assert_ne!(hash7(0), hash7(1));
        // The reference topology: seq 4 is the merge (unlabeled), last is seq 5.
        let merged = render_svg(&p(
            "gitGraph\ncommit\ncommit\nbranch develop\ncheckout develop\ncommit\ncommit\ncheckout main\nmerge develop\ncommit",
        ));
        assert!(merged.contains(">5-"), "last commit is seq 5 (merge consumed 4): {merged}");
        assert!(!merged.contains(">4-"), "D5: merge (seq 4) is unlabeled: {merged}");
        // D2: merge = outer r9 + inner r6.
        assert!(merged.contains(r#"r="9""#) && merged.contains(r#"r="6""#), "merge ring: {merged}");
    }

    #[test]
    fn arrows_palette_rects_and_orthogonal_routing() {
        let svg = render_svg(&p(
            "gitGraph\ncommit\nbranch develop\ncheckout develop\ncommit\ncheckout main\nmerge develop",
        ));
        assert!(svg.contains(r#"stroke-width="8" stroke-linecap="round""#), "D1: {svg}");
        // D9: git0 blue / git1 yellow, resolved from mermaid theme-default.
        assert!(svg.contains("hsl(240, 100%, 71.27%)"), "D9 git0: {svg}");
        assert!(svg.contains("hsl(60, 100%, 68.53%)"), "D9 git1: {svg}");
        assert!(svg.contains(r#"rx="4" ry="4""#), "D6 label rect: {svg}");
        assert!(svg.contains(" Q "), "D8 rounded corner: {svg}");
        assert!(svg.contains(r#"stroke-dasharray="2""#), "D7 branch rule: {svg}");
        assert!(svg.contains("rotate(-45"), "D3 rotated label: {svg}");
    }

    #[test]
    fn branch_checkout_merge() {
        let g = p("gitGraph\ncommit\nbranch develop\ncommit\ncheckout main\ncommit\nmerge develop");
        // commits: 0 main, 1 develop (parent 0), 2 main (parent 0), 3 merge (parents 2 and 1)
        assert_eq!(g.branches, vec!["main", "develop"]);
        assert_eq!(g.commits[1].lane, 1); // develop lane
        assert_eq!(g.commits[1].parents, vec![0]); // branched from main tip
        assert_eq!(g.commits[2].lane, 0);
        assert_eq!(g.commits[2].parents, vec![0]);
        let merge = g.commits.last().unwrap();
        assert_eq!(merge.lane, 0);
        assert_eq!(merge.parents, vec![2, 1]); // current tip + merged tip
    }

    #[test]
    fn commit_options() {
        let g = p("gitGraph\ncommit id: \"init\" tag: \"v1.0\" type: HIGHLIGHT");
        assert_eq!(g.commits[0].id.as_deref(), Some("init"));
        assert_eq!(g.commits[0].tag.as_deref(), Some("v1.0"));
        assert_eq!(g.commits[0].ctype, CommitType::Highlight);
    }

    #[test]
    fn option_key_inside_quoted_value_does_not_match() {
        // `tag: "id: 5"` must set only the tag, not spuriously the id.
        let g = p("gitGraph\ncommit tag: \"id: 5\"");
        assert_eq!(g.commits[0].tag.as_deref(), Some("id: 5"));
        assert!(g.commits[0].id.is_none());
    }

    #[test]
    fn cherry_pick_commits_onto_current_branch() {
        let g = parse(
            "gitGraph\ncommit id: \"A\"\nbranch dev\ncommit id: \"B\"\ncheckout main\ncherry-pick id: \"B\"",
        )
        .unwrap();
        assert_eq!(g.commits.len(), 3);
        let cp = g.commits.last().unwrap();
        assert_eq!(cp.ctype, CommitType::CherryPick);
        assert_eq!(cp.id.as_deref(), Some("B")); // labeled with the picked id
        assert_eq!(cp.lane, 0); // lands on the current branch (main)
        // renders without error
        assert!(crate::render(
            "gitGraph\ncommit id: \"A\"\nbranch dev\ncommit id: \"B\"\ncheckout main\ncherry-pick id: \"B\""
        )
        .svg
        .is_some());
        // a missing `id:` and an unknown id both error loudly
        assert!(parse("gitGraph\ncommit\ncherry-pick").is_err());
        assert!(parse("gitGraph\ncommit\ncherry-pick id: \"nope\"").is_err());
    }

    #[test]
    fn error_paths() {
        assert!(parse("gitGraph").unwrap_err().message.contains("no commits"));
        assert!(parse("gitGraph\ncheckout ghost").unwrap_err().message.contains("unknown branch"));
        assert!(parse("gitGraph\ncommit\nmerge ghost").unwrap_err().message.contains("unknown branch"));
        assert!(parse("gitGraph\ncommit\nbranch main").unwrap_err().message.contains("already exists"));
        assert!(parse("gitGraph\ncherry-pick id: \"x\"").unwrap_err().message.contains("cherry-pick"));
    }

    #[test]
    fn renders_dots_edges_branches() {
        let g = p("gitGraph\ncommit id: \"a\"\nbranch dev\ncommit tag: \"t\"\ncheckout main\ncommit\nmerge dev type: HIGHLIGHT");
        let svg = render_svg(&g);
        assert!(svg.starts_with("<svg") && svg.contains("</svg>"));
        assert!(svg.contains("<circle"), "commit dots");
        assert!(svg.contains("<path"), "edges");
        assert!(svg.contains("main") && svg.contains("dev"), "branch labels");
        assert!(svg.contains(">t<") || svg.contains("t</text>"), "tag");
        assert!(svg.contains("<rect"), "highlight commit");
    }

    #[test]
    fn markup_escaped() {
        let g = p("gitGraph\ncommit tag: \"<x>\"");
        let svg = render_svg(&g);
        assert!(!svg.contains("<x>"));
        assert!(svg.contains("&lt;x&gt;"));
    }

    #[test]
    fn commit_id_label_ends_at_the_dot() {
        // The rotated id label is END-anchored just below its dot so the
        // text trails down-left (upstream behavior); a start anchor would
        // run the text up-right through the dot and lane line.
        let g = p("gitGraph\ncommit id: \"abc\"");
        let svg = render_svg(&g);
        let label = svg
            .split("<text")
            .find(|t| t.contains("abc"))
            .expect("id label present");
        assert!(label.contains(r#"text-anchor="end""#), "label must be end-anchored: {label}");
        assert!(label.contains("rotate(-45"), "label must stay rotated: {label}");
    }

    #[test]
    fn commit_cap_enforced() {
        let mut src = String::from("gitGraph\n");
        for _ in 0..=MAX_COMMITS {
            src.push_str("commit\n");
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }
}
