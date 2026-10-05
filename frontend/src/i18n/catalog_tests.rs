use std::collections::{BTreeMap, BTreeSet};

use fluent_syntax::ast;

use super::*;

fn inline_variables(expr: &ast::InlineExpression<&str>, out: &mut BTreeSet<String>) {
    match expr {
        ast::InlineExpression::VariableReference { id } => {
            out.insert(id.name.to_string());
        }
        ast::InlineExpression::Placeable { expression } => expression_variables(expression, out),
        ast::InlineExpression::FunctionReference { arguments, .. } => {
            argument_variables(arguments, out)
        }
        ast::InlineExpression::TermReference {
            arguments: Some(arguments),
            ..
        } => argument_variables(arguments, out),
        _ => {}
    }
}

fn argument_variables(args: &ast::CallArguments<&str>, out: &mut BTreeSet<String>) {
    for value in &args.positional {
        inline_variables(value, out);
    }
    for value in &args.named {
        inline_variables(&value.value, out);
    }
}

fn expression_variables(expr: &ast::Expression<&str>, out: &mut BTreeSet<String>) {
    match expr {
        ast::Expression::Inline(expr) => inline_variables(expr, out),
        ast::Expression::Select { selector, variants } => {
            inline_variables(selector, out);
            for variant in variants {
                pattern_variables(&variant.value, out);
            }
        }
    }
}

fn pattern_variables(pattern: &ast::Pattern<&str>, out: &mut BTreeSet<String>) {
    for element in &pattern.elements {
        if let ast::PatternElement::Placeable { expression } = element {
            expression_variables(expression, out);
        }
    }
}

/// Map every message/attribute to its arguments using the real Fluent AST.
fn catalog_contract(ftl: &str) -> BTreeMap<String, BTreeSet<String>> {
    let resource = FluentResource::try_new(ftl.to_string()).expect("valid Fluent syntax");
    let mut contract = BTreeMap::new();
    for entry in resource.entries() {
        if let ast::Entry::Message(message) = entry {
            if let Some(value) = &message.value {
                let mut vars = BTreeSet::new();
                pattern_variables(value, &mut vars);
                assert!(
                    contract.insert(message.id.name.to_string(), vars).is_none(),
                    "duplicate {}",
                    message.id.name
                );
            }
            for attribute in &message.attributes {
                let mut vars = BTreeSet::new();
                pattern_variables(&attribute.value, &mut vars);
                let key = format!("{}.{}", message.id.name, attribute.id.name);
                assert!(
                    contract.insert(key.clone(), vars).is_none(),
                    "duplicate {key}"
                );
            }
        }
    }
    contract
}

#[test]
fn every_shipped_catalog_has_the_same_messages_and_arguments() {
    let expected = catalog_contract(EN_US_MAIN_FTL);
    for (locale, _) in LOCALE_CHOICES {
        let id: LanguageIdentifier = locale.parse().unwrap();
        assert_eq!(
            catalog_contract(ftl_for(&id)),
            expected,
            "{locale} catalog differs from en-US"
        );
    }
}

#[test]
fn every_message_formats_without_errors_at_plural_boundaries() {
    let contract = catalog_contract(EN_US_MAIN_FTL);
    for (locale, _) in LOCALE_CHOICES {
        let id: LanguageIdentifier = locale.parse().unwrap();
        // No browser formatter here: syntax, references and plural selection
        // must be valid even on the native CI runner.
        let mut bundle = FluentBundle::new(vec![id.clone()]);
        bundle
            .add_resource(FluentResource::try_new(ftl_for(&id).to_string()).unwrap())
            .unwrap();
        for count in [0, 1, 2, 3, 11, 100, 1_234] {
            for (key, variables) in &contract {
                let mut args = FluentArgs::new();
                for name in variables {
                    args.set(name, count);
                }
                let (message_id, attribute) = key
                    .split_once('.')
                    .map_or((key.as_str(), None), |(k, a)| (k, Some(a)));
                let message = bundle.get_message(message_id).unwrap();
                let pattern = match attribute {
                    Some(name) => message.get_attribute(name).unwrap().value(),
                    None => message.value().unwrap(),
                };
                let mut errors = Vec::new();
                let result = bundle.format_pattern(pattern, Some(&args), &mut errors);
                assert!(
                    errors.is_empty(),
                    "{locale}/{key}, count={count}: {errors:?}"
                );
                assert!(!result.trim().is_empty(), "empty {locale}/{key}");
            }
        }
    }
}

#[test]
fn arabic_replies_use_all_six_plural_categories() {
    let mut bundle = FluentBundle::new(vec![langid!("ar")]);
    bundle
        .add_resource(FluentResource::try_new(AR_MAIN_FTL.to_string()).unwrap())
        .unwrap();
    bundle.set_use_isolating(false);
    let message = bundle.get_message("ss-comment-replies").unwrap();
    for (count, expected) in [
        (0, "لا توجد ردود"),
        (1, "رد واحد"),
        (2, "ردّان"),
        (3, "3 ردود"),
        (11, "11 ردًا"),
        (100, "100 رد"),
    ] {
        let mut args = FluentArgs::new();
        args.set("count", count);
        let mut errors = Vec::new();
        assert_eq!(
            bundle.format_pattern(message.value().unwrap(), Some(&args), &mut errors),
            expected
        );
        assert!(errors.is_empty());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn translation_falls_back_after_formatting_errors_without_browser_logging() {
    struct RestoreBundles(Option<LoadedBundles>);
    impl Drop for RestoreBundles {
        fn drop(&mut self) {
            BUNDLES.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }

    let active = build_bundle(&langid!("de"), "message = { $missing }\n");
    let fallback = build_bundle(&langid!("en-US"), "message = Hello { $name }\n");
    let _restore =
        RestoreBundles(BUNDLES.with(|cell| cell.replace(Some(LoadedBundles { active, fallback }))));
    let mut args = FluentArgs::new();
    args.set("name", "Ada");
    assert_eq!(translate("message", Some(&args)), "Hello Ada");
    assert_eq!(translate("message", None), "message");
    assert_eq!(translate("missing-key", None), "missing-key");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn malformed_catalogs_keep_valid_messages_without_browser_logging() {
    let id = langid!("de");
    let bundle = build_bundle(&id, "valid = Gute Nachricht\nbroken = {\n");
    assert_eq!(
        format_from(&bundle, "valid", None).as_deref(),
        Some("Gute Nachricht")
    );
    let duplicate = build_bundle(&id, "message = first\nmessage = second\n");
    assert!(format_from(&duplicate, "message", None).is_some());
}
