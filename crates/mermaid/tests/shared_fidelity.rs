use ogrenotes_mermaid::render;

#[test]
fn yaml_title_scalars_decode_quotes_escapes_and_comments() {
    for (value, expected) in [
        ("'It''s a diagram'", "It's a diagram"),
        (
            r#""Quotes: \"A\" & \u03B1 \x42 \U0001F600""#,
            "Quotes: \"A\" & α B 😀",
        ),
        ("Research # ignored comment", "Research"),
        ("Research # status: draft", "Research"),
        ("'Title # literal' # ignored comment", "Title # literal"),
        (r#""Line\nBreak""#, "Line\nBreak"),
        (r#""Line\NBreak""#, "Line\nBreak"),
        (r#""Line\LBreak""#, "Line\nBreak"),
        (r#""Line\PBreak""#, "Line\nBreak"),
    ] {
        let svg = render(&format!("---\ntitle: {value}\n---\nflowchart TD\nA --> B"))
            .svg
            .unwrap();
        let mut reader = quick_xml::Reader::from_str(&svg);
        let mut title = None;
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"title" => {
                    title = Some(reader.read_text(tag.name()).unwrap().into_owned());
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        let title = quick_xml::escape::unescape(title.as_deref().unwrap()).unwrap();
        assert_eq!(title, expected, "{value}");
    }
    for value in [
        r#""bad \q""#,
        r#""\uD800""#,
        r#""\U00110000""#,
        r#""\uFFFF""#,
        r#""\uFFFE""#,
        r#""\0""#,
        "\u{ffff}",
        "\u{fffe}",
    ] {
        assert_eq!(
            render(&format!("---\ntitle: {value}\n---\nflowchart TD\nA"))
                .error
                .unwrap()
                .line,
            Some(2)
        );
    }
}

#[test]
fn frontmatter_title_is_visible_escaped_and_accessible() {
    let source =
        "---\ntitle: Research & results\nconfig:\n  theme: default\n---\nflowchart TD\nA --> B";
    let svg = render(source).svg.unwrap();
    assert!(svg.contains("<title>Research &amp; results</title>"));
    let mut reader = quick_xml::Reader::from_str(&svg);
    let mut in_heading = false;
    let mut lines = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"text" => {
                in_heading = tag.attributes().map(Result::unwrap).any(|attr| {
                    attr.key.as_ref() == b"font-weight" && attr.value.as_ref() == b"600"
                });
            }
            quick_xml::events::Event::End(tag) if tag.name().as_ref() == b"text" => {
                in_heading = false
            }
            quick_xml::events::Event::Text(text) if in_heading => {
                lines.push(text.unescape().unwrap().into_owned())
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(lines.concat(), "Research & results");
    let viewbox = svg
        .split("viewBox=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(
        viewbox
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<f64>()
            .unwrap()
            < 0.0,
        "visible title needs its own band above the diagram"
    );
    let svg = render("---\ntitle: Diagram title\n---\npie\naccTitle: Screen reader title\n\"A\":1")
        .svg
        .unwrap();
    assert!(svg.contains("<title>Screen reader title</title>"));
    assert!(svg.contains(">Diagram title</text>"));
}

#[test]
fn multiline_configuration_comments_preserve_error_lines() {
    let source = "%%{init: {\n\"theme\": \"default\"\n}}%%\nflowchart TD\nA --> B";
    assert!(render(source).svg.is_some());
    let source = "%%{init: {\n\"theme\": \"default\"\n}}%%\nflowchart TD\nA[unclosed";
    assert_eq!(render(source).error.unwrap().line, Some(5));
    let source = "flowchart TD\n%%{init: {\nmissing closing comment";
    assert_eq!(render(source).error.unwrap().line, Some(2));
}

#[test]
fn frontmatter_complex_or_unclosed_titles_fail_with_original_lines() {
    for source in [
        "---\ntitle: |\n  multiline\n---\npie\n\"a\":1",
        "---\ntitle: \"unclosed\n---\npie\n\"a\":1",
    ] {
        assert_eq!(render(source).error.unwrap().line, Some(2));
    }
}

#[test]
fn decoded_title_newlines_are_visible_even_when_the_whole_title_would_fit() {
    for escape in [r"\n", r"\N", r"\L", r"\P"] {
        let source=format!("---\ntitle: \"Line{escape}Break\"\n---\nflowchart LR\nA[A sufficiently wide diagram node] --> B");
        let svg = render(&source).svg.unwrap();
        assert!(svg.contains(">Line</text>"));
        assert!(svg.contains(">Break</text>"));
        assert!(!svg.contains(">Line Break</text>"));
    }
    let svg=render("---\ntitle: \"Literal <br/> title\"\n---\nflowchart LR\nA[A sufficiently wide diagram node] --> B").svg.unwrap();
    assert!(svg.contains("Literal &lt;br/&gt; title</text>"));
}
