//! Renders the example snippets from the official mermaid state-diagram
//! docs (https://mermaid.ai/open-source/syntax/stateDiagram.html, source
//! packages/mermaid/src/docs/syntax/stateDiagram.md) through our renderer,
//! writing one .svg (success) or .err.txt (error) per example into
//! target/state-gallery/ for side-by-side comparison with the docs.
//!
//! Run: cargo run -p ogrenotes-mermaid --example state_gallery
//!
//! Doc cases doc01–doc20 are the page's 20 mermaid blocks in page order;
//! probe_* cases exercise the state-polish edge fixes. Expectation notes
//! reflect the post-state-polish behavior
//! (docs/superpowers/specs/2026-07-11-mermaid-state-polish-design.md).

use std::fs;
use std::path::Path;

fn main() {
    // (name, source, expectation-note)
    let cases: &[(&str, &str, &str)] = &[
        // ── Doc examples (page order) ────────────────────────────
        ("doc01_intro",
         "---\ntitle: Simple sample\n---\nstateDiagram-v2\n    [*] --> Still\n    Still --> [*]\n\n    Still --> Moving\n    Moving --> Still\n    Moving --> Crash\n    Crash --> [*]\n",
         "match: front-matter title not rendered (quiet cosmetic)"),
        ("doc02_v1_header",
         "stateDiagram\n    [*] --> Still\n    Still --> [*]\n\n    Still --> Moving\n    Moving --> Still\n    Moving --> Crash\n    Crash --> [*]\n",
         "match"),
        ("doc03_bare_state_id",
         "stateDiagram-v2\n    stateId\n",
         "match (bare-id declarations added in state-polish)"),
        ("doc04_state_as",
         "stateDiagram-v2\n    state \"This is a state description\" as s2\n",
         "match"),
        ("doc05_colon_description",
         "stateDiagram-v2\n    s2 : This is a state description\n",
         "match (colon descriptions added in state-polish)"),
        ("doc06_transition",
         "stateDiagram-v2\n    s1 --> s2\n",
         "match"),
        ("doc07_transition_label",
         "stateDiagram-v2\n    s1 --> s2: A transition\n",
         "match"),
        ("doc08_start_end",
         "stateDiagram-v2\n    [*] --> s1\n    s1 --> [*]\n",
         "match"),
        ("doc09_composite",
         "stateDiagram-v2\n    [*] --> First\n    state First {\n        [*] --> second\n        second --> [*]\n    }\n\n    [*] --> NamedComposite\n    NamedComposite: Another Composite\n    state NamedComposite {\n        [*] --> namedSimple\n        namedSimple --> [*]\n        namedSimple: Another simple\n    }\n",
         "error expected (composite used as a transition endpoint — routing deferred, issue #47)"),
        ("doc10_composite_nested",
         "stateDiagram-v2\n    [*] --> First\n\n    state First {\n        [*] --> Second\n\n        state Second {\n            [*] --> second\n            second --> Third\n\n            state Third {\n                [*] --> third\n                third --> [*]\n            }\n        }\n    }\n",
         "error expected (composite used as a transition endpoint — issue #47)"),
        ("doc11_composite_transitions",
         "stateDiagram-v2\n    [*] --> First\n    First --> Second\n    First --> Third\n\n    state First {\n        [*] --> fir\n        fir --> [*]\n    }\n    state Second {\n        [*] --> sec\n        sec --> [*]\n    }\n    state Third {\n        [*] --> thi\n        thi --> [*]\n    }\n",
         "error expected (composite used as a transition endpoint — issue #47)"),
        ("doc12_choice",
         "stateDiagram-v2\n    state if_state <<choice>>\n    [*] --> IsPositive\n    IsPositive --> if_state\n    if_state --> False: if n < 0\n    if_state --> True : if n >= 0\n",
         "diverge: we inscribe the id in the diamond; mermaid draws it small and empty"),
        ("doc13_fork_join",
         "   stateDiagram-v2\n    state fork_state <<fork>>\n      [*] --> fork_state\n      fork_state --> State2\n      fork_state --> State3\n\n      state join_state <<join>>\n      State2 --> join_state\n      State3 --> join_state\n      join_state --> State4\n      State4 --> [*]\n",
         "match"),
        ("doc14_notes",
         "    stateDiagram-v2\n        State1: The state with a note\n        note right of State1\n            Important information! You can write\n            notes.\n        end note\n        State1 --> State2\n        note left of State2 : This is the note to the left.\n",
         "error expected (multi-line notes unsupported; single-line colon notes now render on-canvas)"),
        ("doc15_concurrency",
         "stateDiagram-v2\n    [*] --> Active\n\n    state Active {\n        [*] --> NumLockOff\n        NumLockOff --> NumLockOn : EvNumLockPressed\n        NumLockOn --> NumLockOff : EvNumLockPressed\n        --\n        [*] --> CapsLockOff\n        CapsLockOff --> CapsLockOn : EvCapsLockPressed\n        CapsLockOn --> CapsLockOff : EvCapsLockPressed\n        --\n        [*] --> ScrollLockOff\n        ScrollLockOff --> ScrollLockOn : EvScrollLockPressed\n        ScrollLockOn --> ScrollLockOff : EvScrollLockPressed\n    }\n",
         "error expected (already-a-state guard fires on `Active` before the `--` divider is reached)"),
        ("doc16_direction",
         "stateDiagram\n    direction LR\n    [*] --> A\n    A --> B\n    B --> C\n    state B {\n      direction LR\n      a --> b\n    }\n    B --> D\n",
         "error expected (`direction` out of scope)"),
        ("doc17_comments",
         "stateDiagram-v2\n    [*] --> Still\n    Still --> [*]\n%% this is a comment\n    Still --> Moving\n    Moving --> Still %% another comment\n    Moving --> Crash\n    Crash --> [*]\n",
         "match (trailing %% comments added in state-polish)"),
        ("doc18_classdef_class",
         "   stateDiagram\n   direction TB\n\n   accTitle: This is the accessible title\n   accDescr: This is an accessible description\n\n   classDef notMoving fill:white\n   classDef movement font-style:italic\n   classDef badBadEvent fill:#f00,color:white,font-weight:bold,stroke-width:2px,stroke:yellow\n\n   [*]--> Still\n   Still --> [*]\n   Still --> Moving\n   Moving --> Still\n   Moving --> Crash\n   Crash --> [*]\n\n   class Still notMoving\n   class Moving, Crash movement\n   class Crash badBadEvent\n   class end badBadEvent\n",
         "error expected, naming direction (first unsupported statement, line 2 — before accTitle/accDescr/classDef/class)"),
        ("doc19_classdef_colon_operator",
         "stateDiagram\n   direction TB\n\n   accTitle: This is the accessible title\n   accDescr: This is an accessible description\n\n   classDef notMoving fill:white\n   classDef movement font-style:italic;\n   classDef badBadEvent fill:#f00,color:white,font-weight:bold,stroke-width:2px,stroke:yellow\n\n   [*] --> Still:::notMoving\n   Still --> [*]\n   Still --> Moving:::movement\n   Moving --> Still\n   Moving --> Crash:::movement\n   Crash:::badBadEvent --> [*]\n",
         "error expected, naming direction (line 2 — comes before ::: in this source)"),
        ("doc20_spaces_in_names",
         "stateDiagram\n    classDef yourState font-style:italic,font-weight:bold,fill:white\n\n    yswsii: Your state with spaces in it\n    [*] --> yswsii:::yourState\n    [*] --> SomeOtherState\n    SomeOtherState --> YetAnotherState\n    yswsii --> YetAnotherState\n    YetAnotherState --> [*]\n",
         "error expected, naming classDef"),
        // ── Probes (state-polish edge fixes) ─────────────────────
        ("probe_ccc_target",
         "stateDiagram-v2\n[*] --> Still:::notMoving",
         "error expected, naming ::: (fixed silent misparse)"),
        ("probe_ccc_source",
         "stateDiagram-v2\nStill:::notMoving --> [*]",
         "error expected (source-side ::: surfaces as a transition-parse error)"),
        ("probe_note_left",
         "stateDiagram-v2\nState1 --> State2\nnote left of State2 : This is the note to the left.",
         "match: note on-canvas (fixed silent divergence)"),
        ("probe_note_right",
         "stateDiagram-v2\nA --> B\nnote right of A: on the right",
         "match: note on-canvas"),
        ("probe_composite_unused",
         "stateDiagram-v2\nstate X {\na --> b\n}",
         "match: composite never used as endpoint"),
        ("probe_composite_endpoint",
         "stateDiagram-v2\nc --> X\nstate X {\na --> b\n}",
         "error expected (endpoint before composite decl → already-a-state guard)"),
        ("probe_nospace_start",
         "stateDiagram-v2\n[*]--> A",
         "match: no-space [*] arrow"),
        ("probe_label_semicolon",
         "stateDiagram-v2\na --> b: go;",
         "match: `;` stays in the label to end of line (fixed in the final wave; supersedes the #32 truncation item)"),
        ("probe_label_semicolon_tail",
         "stateDiagram-v2\na --> b: go; Stop",
         "match: ONE edge labeled 'go; Stop', no phantom node (final-review regression fix)"),
        ("probe_semicolon_split",
         "stateDiagram-v2\na --> b; c --> d",
         "match: ; splits statements"),
        ("probe_header_chain",
         "stateDiagram-v2; s1 --> s2",
         "error expected (header cannot chain — filed on #32)"),
        ("probe_synthetic_note",
         "stateDiagram-v2\n[*] --> A\nnote right of __start_0: boo",
         "error expected, naming synthetic (fixed phantom)"),
        ("probe_bare_dashdash",
         "stateDiagram-v2\na --> b\n--",
         "error expected, naming --"),
        ("probe_desc_stacking",
         "stateDiagram-v2\ns : first\ns : second",
         "match: repeated descriptions stack"),
        ("probe_single_percent",
         "stateDiagram-v2\na --> b: 50% done",
         "match: single % survives"),
        ("probe_classdef_named",
         "stateDiagram-v2\na --> b\nclassDef x fill:red",
         "error expected, naming classDef"),
        ("probe_class_named",
         "stateDiagram-v2\na --> b\nclass a b",
         "error expected, naming class"),
        ("probe_acctitle_named",
         "stateDiagram-v2\na --> b\naccTitle: t",
         "error expected, naming accTitle"),
        ("probe_accdescr_named",
         "stateDiagram-v2\na --> b\naccDescr: d",
         "error expected, naming accDescr"),
    ];

    let out_dir = Path::new("target/state-gallery");
    fs::create_dir_all(out_dir).expect("create out dir");

    let (mut ok, mut err) = (0usize, 0usize);
    for (name, source, note) in cases {
        let result = ogrenotes_mermaid::render(source);
        match (result.svg, result.error) {
            (Some(svg), None) => {
                fs::write(out_dir.join(format!("{name}.svg")), svg).expect("write svg");
                ok += 1;
                println!("RENDERED  {name:<28} — {note}");
            }
            (None, Some(e)) => {
                let msg = format!(
                    "line {:?}: {}\n\nsource:\n{}\n\nexpectation: {}\n",
                    e.line, e.message, source, note
                );
                fs::write(out_dir.join(format!("{name}.err.txt")), msg).expect("write err");
                err += 1;
                println!("ERRORED   {name:<28} — {note}");
            }
            other => {
                // XOR invariant means this is unreachable; keep loud anyway.
                println!("INVARIANT VIOLATION {name}: {other:?}");
            }
        }
    }
    println!("\n{ok} rendered, {err} errored → target/state-gallery/");
}
