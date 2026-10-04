#![cfg(target_arch = "wasm32")]

use fluent_bundle::FluentArgs;
use ogrenotes_frontend::i18n;
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn numeric_arguments_keep_plural_rules_and_localized_digits() {
    i18n::init("ar-EG");
    for (count, expected) in [
        (0, "لا توجد ردود"),
        (1, "رد واحد"),
        (2, "ردّان"),
        (3, "٣ ردود"),
        (11, "١١ ردًا"),
        (100, "١٠٠ رد"),
    ] {
        let mut args = FluentArgs::new();
        args.set("count", count);
        assert_eq!(i18n::translate("ss-comment-replies", Some(&args)), expected);
    }
    i18n::init("de-DE");
    assert_eq!(
        i18n::format_number_with_precision(1234.56789, 4),
        "1.234,5679"
    );
    let mut args = FluentArgs::new();
    args.set("count", 1234);
    assert_eq!(
        i18n::translate("ss-comment-replies", Some(&args)),
        "1.234 Antworten"
    );
    i18n::init("en-US");
}

#[wasm_bindgen_test]
fn fluent_number_arguments_preserve_explicit_format_options() {
    use fluent_bundle::types::{
        FluentNumber, FluentNumberCurrencyDisplayStyle, FluentNumberOptions, FluentNumberStyle,
    };
    i18n::init("de-DE");
    for (value, options, expected) in [
        (
            1234.0,
            FluentNumberOptions {
                use_grouping: false,
                minimum_fraction_digits: Some(2),
                maximum_fraction_digits: Some(2),
                ..Default::default()
            },
            "1234,00 Antworten",
        ),
        (
            1234.0,
            FluentNumberOptions {
                style: FluentNumberStyle::Currency,
                currency: Some("EUR".into()),
                currency_display: FluentNumberCurrencyDisplayStyle::Code,
                ..Default::default()
            },
            "1.234,00\u{a0}EUR Antworten",
        ),
        (
            0.125,
            FluentNumberOptions {
                style: FluentNumberStyle::Percent,
                maximum_fraction_digits: Some(1),
                ..Default::default()
            },
            "12,5\u{a0}% Antworten",
        ),
        (
            1234.56,
            FluentNumberOptions {
                minimum_significant_digits: Some(3),
                maximum_significant_digits: Some(3),
                ..Default::default()
            },
            "1.230 Antworten",
        ),
        (
            1234.0,
            FluentNumberOptions {
                use_grouping: false,
                minimum_integer_digits: Some(6),
                ..Default::default()
            },
            "001234 Antworten",
        ),
    ] {
        let mut args = FluentArgs::new();
        args.set("count", FluentNumber::new(value, options));
        assert_eq!(i18n::translate("ss-comment-replies", Some(&args)), expected);
    }
    i18n::init("en-US");
}

#[wasm_bindgen_test]
fn invalid_number_options_fall_back_without_a_browser_exception() {
    use fluent_bundle::types::{FluentNumber, FluentNumberOptions, FluentNumberStyle};
    i18n::init("de-DE");
    for options in [
        FluentNumberOptions {
            style: FluentNumberStyle::Currency,
            ..Default::default()
        },
        FluentNumberOptions {
            minimum_fraction_digits: Some(5),
            maximum_fraction_digits: Some(2),
            ..Default::default()
        },
        FluentNumberOptions {
            minimum_significant_digits: Some(0),
            ..Default::default()
        },
    ] {
        let mut args = FluentArgs::new();
        args.set("count", FluentNumber::new(1234.0, options));
        assert_eq!(
            i18n::translate("ss-comment-replies", Some(&args)),
            "1.234 Antworten"
        );
    }
    assert_eq!(
        i18n::format_number_with_precision(1234.56789, 255),
        "1234.56789"
    );
    i18n::init("en-US");
}
