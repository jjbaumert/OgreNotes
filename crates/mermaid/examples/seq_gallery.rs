//! Renders the example snippets from the official mermaid sequence-diagram
//! docs (https://mermaid.ai/open-source/syntax/sequenceDiagram.html, source
//! packages/mermaid/src/docs/syntax/sequenceDiagram.md) through our
//! renderer, writing one .svg (success) or .err.txt (error) per example
//! into target/seq-gallery/ for side-by-side comparison with the docs.
//!
//! Run: cargo run -p ogrenotes-mermaid --example seq_gallery
//!
//! Expectation notes reflect the post-sequence-polish behavior (issue #45;
//! docs/superpowers/specs/2026-07-11-mermaid-sequence-polish-design.md).

use std::fs;
use std::path::Path;

fn main() {
    // (name, source, expectation-note)
    let cases: &[(&str, &str, &str)] = &[
        // ── Participants / actors ────────────────────────────────
        ("intro_basic",
         "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    Alice-)John: See you later!",
         "match"),
        ("participants_explicit",
         "sequenceDiagram\n    participant Alice\n    participant Bob\n    Bob->>Alice: Hi Alice\n    Alice->>Bob: Hi Bob",
         "match: declaration order wins"),
        ("actors",
         "sequenceDiagram\n    actor Alice\n    actor Bob\n    Alice->>Bob: Hi Bob\n    Bob->>Alice: Hi Alice",
         "match: stick figures"),
        ("stereo_boundary",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"boundary\" }\n    participant Bob\n    Alice->>Bob: Request from boundary\n    Bob->>Alice: Response to boundary",
         "error expected (stereotype @-syntax out of scope)"),
        ("stereo_control",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"control\" }\n    participant Bob\n    Alice->>Bob: Control request\n    Bob->>Alice: Control response",
         "error expected"),
        ("stereo_entity",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"entity\" }\n    participant Bob\n    Alice->>Bob: Entity request\n    Bob->>Alice: Entity response",
         "error expected"),
        ("stereo_database",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"database\" }\n    participant Bob\n    Alice->>Bob: DB query\n    Bob->>Alice: DB result",
         "error expected"),
        ("stereo_collections",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"collections\" }\n    participant Bob\n    Alice->>Bob: Collections request\n    Bob->>Alice: Collections response",
         "error expected"),
        ("stereo_queue",
         "sequenceDiagram\n    participant Alice@{ \"type\" : \"queue\" }\n    participant Bob\n    Alice->>Bob: Queue message\n    Bob->>Alice: Queue response",
         "error expected"),
        ("alias_external",
         "sequenceDiagram\n    participant A as Alice\n    participant J as John\n    A->>J: Hello John, how are you?\n    J->>A: Great!",
         "match"),
        ("alias_external_stereo",
         "sequenceDiagram\n    participant API@{ \"type\": \"boundary\" } as Public API\n    actor DB@{ \"type\": \"database\" } as User Database\n    participant Svc@{ \"type\": \"control\" } as Auth Service\n    API->>Svc: Authenticate\n    Svc->>DB: Query user\n    DB-->>Svc: User data\n    Svc-->>API: Token",
         "error expected (stereotype)"),
        ("alias_inline_stereo",
         "sequenceDiagram\n    participant API@{ \"type\": \"boundary\", \"alias\": \"Public API\" }\n    participant Auth@{ \"type\": \"control\", \"alias\": \"Auth Service\" }\n    participant DB@{ \"type\": \"database\", \"alias\": \"User Database\" }\n    API->>Auth: Login request\n    Auth->>DB: Query user\n    DB-->>Auth: User data\n    Auth-->>API: Access token",
         "error expected"),
        ("alias_precedence",
         "sequenceDiagram\n    participant API@{ \"type\": \"boundary\", \"alias\": \"Internal Name\" } as External Name\n    participant DB@{ \"type\": \"database\", \"alias\": \"Internal DB\" } as External DB\n    API->>DB: Query\n    DB-->>API: Result",
         "error expected"),
        ("create_destroy",
         "sequenceDiagram\n    Alice->>Bob: Hello Bob, how are you ?\n    Bob->>Alice: Fine, thank you. And you?\n    create participant Carl\n    Alice->>Carl: Hi Carl!\n    create actor D as Donald\n    Carl->>D: Hi!\n    destroy Carl\n    Alice-xCarl: We are too many\n    destroy Bob\n    Bob->>Alice: I agree",
         "error expected (`create` out of scope)"),
        ("create_snippet",
         "sequenceDiagram\n    create participant B\n    A --> B: Hello",
         "error expected (`create`)"),
        ("box_purple",
         "sequenceDiagram\n    box Purple Alice & John\n    participant A\n    participant J\n    end\n    box Another Group\n    participant B\n    participant C\n    end\n    A->>J: Hello John, how are you?\n    J->>A: Great!\n    A->>B: Hello Bob, how is Charley?\n    B->>C: Hello Charley, how are you?",
         "error expected (`box` out of scope)"),
        ("box_rgb",
         "sequenceDiagram\n    box rgb(33,66,99)\n    participant A\n    end\n    A->>A: hi",
         "error expected (`box`)"),
        ("box_transparent",
         "sequenceDiagram\n    box transparent Aqua\n    participant A\n    end\n    A->>A: hi",
         "error expected (`box`)"),
        ("central_conn_target",
         "sequenceDiagram\n    participant Alice\n    participant John\n    Alice->>()John: Hello John",
         "error expected, naming central connections (added in seq-polish)"),
        ("central_conn_source",
         "sequenceDiagram\n    participant Alice\n    participant John\n    Alice()->>John: How are you?",
         "error expected, naming central connections"),
        ("central_conn_both",
         "sequenceDiagram\n    participant Alice\n    participant John\n    John()->>()Alice: Great!",
         "error expected, naming central connections"),
        // ── Messages / arrows (doc table) ────────────────────────
        ("arrow_solid_noarrow",   "sequenceDiagram\n    A->B: solid open",    "match: plain line, no marker"),
        ("arrow_dotted_noarrow",  "sequenceDiagram\n    A-->B: dotted open",  "match"),
        ("arrow_solid_head",      "sequenceDiagram\n    A->>B: solid head",   "match"),
        ("arrow_dotted_head",     "sequenceDiagram\n    A-->>B: dotted head", "match"),
        ("arrow_bidi_solid",      "sequenceDiagram\n    A<<->>B: bidirectional", "match: markers both ends (added in seq-polish)"),
        ("arrow_bidi_dotted",     "sequenceDiagram\n    A<<-->>B: bidirectional dotted", "match (added in seq-polish)"),
        ("arrow_cross_solid",     "sequenceDiagram\n    A-xB: cross",         "match: X marker"),
        ("arrow_cross_dotted",    "sequenceDiagram\n    A--xB: cross dotted", "match"),
        ("arrow_async_solid",     "sequenceDiagram\n    A-)B: async open",    "match: open-V marker"),
        ("arrow_async_dotted",    "sequenceDiagram\n    A--)B: async dotted", "match"),
        // half-arrows (v11.12.3 doc table; representative spellings)
        ("half_arrow_top",        "sequenceDiagram\n    A-|\\B: top half",    "error expected, naming half arrows (out of scope)"),
        ("half_arrow_bottom",     "sequenceDiagram\n    A-|/B: bottom half",  "error expected, naming half arrows"),
        ("half_arrow_reverse",    "sequenceDiagram\n    A/|-B: reverse top",  "error expected, naming half arrows"),
        ("half_arrow_stick",      "sequenceDiagram\n    A-\\\\B: top stick",  "error expected, naming half arrows"),
        ("half_arrow_stick_dotted","sequenceDiagram\n    A--//B: bottom stick dotted", "error expected, naming half arrows"),
        // semicolon as statement separator (doc: use #59; for a literal ;)
        ("msg_semicolon_separator",
         "sequenceDiagram\n    A->>B: hi; B-->>A: yo",
         "match: TWO messages — `;` is a statement separator (fixed in seq-polish; was one message with the tail inside its text)"),
        // ── Activations ──────────────────────────────────────────
        ("act_explicit",
         "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    activate John\n    John-->>Alice: Great!\n    deactivate John",
         "match"),
        ("act_shorthand",
         "sequenceDiagram\n    Alice->>+John: Hello John, how are you?\n    John-->>-Alice: Great!",
         "match"),
        ("act_stacked",
         "sequenceDiagram\n    Alice->>+John: Hello John, how are you?\n    Alice->>+John: John, can you hear me?\n    John-->>-Alice: Hi Alice, I can hear you!\n    John-->>-Alice: I feel great!",
         "match: stacked activation bars"),
        ("act_spaced_shorthand",
         "sequenceDiagram\n    Alice ->>+ John: Did you want to go to the game tonight?\n    John -->>- Alice: Yeah! See you there.",
         "match: spaced spelling accepted (added in seq-polish)"),
        // ── Notes / line breaks ──────────────────────────────────
        ("note_right",
         "sequenceDiagram\n    participant John\n    Note right of John: Text in note",
         "match"),
        ("note_over_two",
         "sequenceDiagram\n    Alice->John: Hello John, how are you?\n    Note over Alice,John: A typical interaction",
         "match: spans both lifelines"),
        ("note_over_three",
         "sequenceDiagram\n    Note over A,B,C: spans three?",
         "match: all three interned, note spans outermost lifelines (fixed in seq-polish; was silent drop, #32)"),
        ("linebreak_msg_note",
         "sequenceDiagram\n    Alice->John: Hello John,<br/>how are you?\n    Note over Alice,John: A typical interaction<br/>But now in two lines",
         "match: <br/> in message and note text -> tspans"),
        ("linebreak_actor_alias",
         "sequenceDiagram\n    participant Alice as Alice<br/>Johnson\n    Alice->John: Hello John,<br/>how are you?\n    Note over Alice,John: A typical interaction<br/>But now in two lines",
         "match: participant display renders per-line tspans (fixed in seq-polish; was literal, #32)"),
        // ── Fragments ────────────────────────────────────────────
        ("loop_basic",
         "sequenceDiagram\n    Alice->John: Hello John, how are you?\n    loop Every minute\n        John-->Alice: Great!\n    end",
         "match"),
        ("alt_opt",
         "sequenceDiagram\n    Alice->>Bob: Hello Bob, how are you?\n    alt is sick\n        Bob->>Alice: Not so good :(\n    else is well\n        Bob->>Alice: Feeling fresh like a daisy\n    end\n    opt Extra response\n        Bob->>Alice: Thanks for asking\n    end",
         "match"),
        ("par_basic",
         "sequenceDiagram\n    par Alice to Bob\n        Alice->>Bob: Hello guys!\n    and Alice to John\n        Alice->>John: Hello guys!\n    end\n    Bob-->>Alice: Hi Alice!\n    John-->>Alice: Hi Alice!",
         "match"),
        ("par_nested",
         "sequenceDiagram\n    par Alice to Bob\n        Alice->>Bob: Go help John\n    and Alice to John\n        Alice->>John: I want this done today\n        par John to Charlie\n            John->>Charlie: Can we do this today?\n        and John to Diana\n            John->>Diana: Can you help us today?\n        end\n    end",
         "match: nested par"),
        ("critical_options",
         "sequenceDiagram\n    critical Establish a connection to the DB\n        Service-->DB: connect\n    option Network timeout\n        Service-->Service: Log error\n    option Credentials rejected\n        Service-->Service: Log different error\n    end",
         "match: option dividers (added in seq-polish)"),
        ("critical_bare",
         "sequenceDiagram\n    critical Establish a connection to the DB\n        Service-->DB: connect\n    end",
         "match"),
        ("break_basic",
         "sequenceDiagram\n    Consumer-->API: Book something\n    API-->BookingService: Start booking process\n    break when the booking process fails\n        API-->Consumer: show failure\n    end\n    API-->BillingService: Start billing process",
         "match"),
        ("rect_highlight",
         "sequenceDiagram\n    participant Alice\n    participant John\n\n    rect rgb(191, 223, 255)\n    note right of Alice: Alice calls John.\n    Alice->>+John: Hello John, how are you?\n    rect rgb(200, 150, 255)\n    Alice->>+John: John, can you hear me?\n    John-->>-Alice: Hi Alice, I can hear you!\n    end\n    John-->>-Alice: I feel great!\n    end\n    Alice ->>+ John: Did you want to go to the game tonight?\n    John -->>- Alice: Yeah! See you there.",
         "error expected (`rect` out of scope; the spaced shorthand inside is supported since seq-polish but `rect` errors first)"),
        ("rect_rgba",
         "sequenceDiagram\n    rect rgba(0, 0, 255, .1)\n    A->>B: x\n    end",
         "error expected (`rect`)"),
        ("comments",
         "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    %% this is a comment\n    John-->>Alice: Great!",
         "match: %% lines skipped"),
        // ── Escaping / autonumber / menus ────────────────────────
        ("entity_codes",
         "sequenceDiagram\n    A->>B: I #9829; you!\n    B->>A: I #9829; you #infin; times more!",
         "diverge: entity codes render literally, not decoded (their `;` does NOT split — entity guard)"),
        ("entity_semicolon",
         "sequenceDiagram\n    A->>B: hi#59;there",
         "diverge: #59; renders literally instead of `;` — but stays ONE message (entity guard on the splitter)"),
        ("autonumber_basic",
         "sequenceDiagram\n    autonumber\n    Alice->>John: Hello John, how are you?\n    loop HealthCheck\n        John->>John: Fight against hypochondria\n    end\n    Note right of John: Rational thoughts!\n    John-->>Alice: Great!\n    John->>Bob: How about you?\n    Bob-->>John: Jolly good!",
         "match: numbers on arrows incl. self-message; rendered as inline label prefix (mermaid draws a boxed number)"),
        ("autonumber_args",
         "sequenceDiagram\n    autonumber 10 10\n    Alice->>John: Hello\n    John-->>Alice: Hi",
         "error expected (start/increment args deliberately refused, v11.15 syntax)"),
        ("autonumber_off",
         "sequenceDiagram\n    autonumber\n    A->>B: one\n    autonumber off\n    B-->>A: two",
         "error expected (autonumber args refused)"),
        ("link_menu",
         "sequenceDiagram\n    participant Alice\n    participant John\n    link Alice: Dashboard @ https://dashboard.contoso.com/alice\n    link Alice: Wiki @ https://wiki.contoso.com/alice\n    link John: Dashboard @ https://dashboard.contoso.com/john\n    link John: Wiki @ https://wiki.contoso.com/john\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    Alice-)John: See you later!",
         "error expected (`link` out of scope)"),
        ("links_json",
         "sequenceDiagram\n    participant Alice\n    participant John\n    links Alice: {\"Dashboard\": \"https://dashboard.contoso.com/alice\", \"Wiki\": \"https://wiki.contoso.com/alice\"}\n    links John: {\"Dashboard\": \"https://dashboard.contoso.com/john\", \"Wiki\": \"https://wiki.contoso.com/john\"}\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    Alice-)John: See you later!",
         "error expected (`links` out of scope)"),
    ];

    let out_dir = Path::new("target/seq-gallery");
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
    println!("\n{ok} rendered, {err} errored → target/seq-gallery/");
}
