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
const COMMIT_GAP: f64 = 46.0;
const LANE_GAP: f64 = 44.0;
const TOP: f64 = 26.0;
const DOT_R: f64 = 7.0;

const LANE_COLORS: &[&str] = &[
    "#4A90D9", "#5C3D2E", "#5CB85C", "#D9534F", "#9B59B6", "#F0AD4E", "#2D5F2D", "#E67E22",
];

fn lane_color(lane: usize) -> &'static str {
    LANE_COLORS[lane % LANE_COLORS.len()]
}

/// A deterministic short hash-like id for a commit that wasn't given an
/// explicit `id:`, mirroring Mermaid's generated commit hashes. Stable across
/// runs (the crate contract forbids nondeterministic output).
fn auto_id(seq: usize) -> String {
    // Cheap integer scramble → 7 hex chars, so ids look hash-like but stable.
    let mut h = (seq as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    format!("{:07x}", h & 0xFFF_FFFF)
}

pub(crate) fn render_svg(g: &GitGraph) -> String {
    let max_seq = g.commits.iter().map(|c| c.seq).max().unwrap_or(0);
    let n_lanes = g.branches.len();
    let x_of = |seq: usize| LABEL_W + 20.0 + seq as f64 * COMMIT_GAP;
    let y_of = |lane: usize| TOP + lane as f64 * LANE_GAP;
    let w = x_of(max_seq) + 40.0;
    let h = y_of(n_lanes.saturating_sub(1)) + 30.0;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:12px">"#
    );

    // Edges first (under the dots): parent -> commit, colored by the
    // outermost lane involved (so branch/merge lines take the feature
    // branch's colour), curved along the horizontal flow.
    for c in &g.commits {
        let (cx, cy) = (x_of(c.seq), y_of(c.lane));
        for &p in &c.parents {
            let pc = &g.commits[p];
            let (px, py) = (x_of(pc.seq), y_of(pc.lane));
            let color = lane_color(c.lane.max(pc.lane));
            let d = crate::curved_path(&[(px, py), (cx, cy)], false);
            svg.push_str(&format!(
                r#"<path d="{d}" stroke="{color}" stroke-width="2" fill="none"/>"#
            ));
        }
    }

    // Branch labels at the left, on each lane baseline.
    for (lane, name) in g.branches.iter().enumerate() {
        svg.push_str(&format!(
            r#"<text x="6" y="{:.1}" fill="{}" font-weight="600">{}</text>"#,
            y_of(lane) + 4.0,
            lane_color(lane),
            escape_xml(name),
        ));
    }

    // Commit dots + tag/id labels.
    for c in &g.commits {
        let (cx, cy) = (x_of(c.seq), y_of(c.lane));
        let color = lane_color(c.lane);
        // A commit with two+ parents is a merge; Mermaid rings it distinctly.
        let is_merge = c.parents.len() >= 2;
        match c.ctype {
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
                // Filled dot with a small cross, so a cherry-pick reads
                // distinctly from a normal commit.
                let a = DOT_R * 0.55;
                svg.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{DOT_R}" fill="{color}"/><path d="M {:.1} {:.1} L {:.1} {:.1} M {:.1} {:.1} L {:.1} {:.1}" stroke="var(--surface, #fff)" stroke-width="1.5"/>"#,
                    cx - a, cy, cx + a, cy, cx, cy - a, cx, cy + a,
                ));
            }
        }
        // Merge commits get an outer ring so they read distinctly from a
        // normal commit on the same lane.
        if is_merge {
            svg.push_str(&format!(
                r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{:.1}" fill="none" stroke="{color}" stroke-width="1.5"/>"#,
                DOT_R + 3.0
            ));
        }
        if let Some(tag) = &c.tag {
            svg.push_str(&format!(
                r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" style="font-weight:600">{}</text>"#,
                cy - DOT_R - 4.0,
                escape_xml(tag),
            ));
        }
        // Every commit carries an id label below its dot (Mermaid shows a short
        // generated hash); an explicit `id:` wins, else a deterministic one.
        let label = c.id.clone().unwrap_or_else(|| auto_id(c.seq));
        svg.push_str(&format!(
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" style="font-size:10px">{}</text>"#,
            cy + DOT_R + 12.0,
            escape_xml(&label),
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
    fn every_commit_gets_an_id_label_and_merges_are_ringed() {
        // Auto ids: two plain commits render two id labels even without `id:`.
        let plain = render_svg(&p("gitGraph\ncommit\ncommit"));
        assert_eq!(plain.matches("font-size:10px").count(), 2, "one auto id per commit");
        assert_ne!(auto_id(0), auto_id(1), "auto ids differ per commit");
        // A merge commit gets an extra unfilled ring circle.
        let merged =
            render_svg(&p("gitGraph\ncommit\nbranch dev\ncommit\ncheckout main\nmerge dev"));
        assert!(merged.contains(r#"fill="none""#), "merge commit ringed: {merged}");
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
    fn commit_cap_enforced() {
        let mut src = String::from("gitGraph\n");
        for _ in 0..=MAX_COMMITS {
            src.push_str("commit\n");
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }
}
