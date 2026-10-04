#!/usr/bin/env python3
"""Regression tests for source lint boundaries and interpolated messages."""
import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('i18n_audit', Path(__file__).with_name('i18n-audit.py'))
audit = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = audit
spec.loader.exec_module(audit)


class SourceAuditTests(unittest.TestCase):
    def values(self, source):
        return [value for _, _, value in audit.audit_source(source)]

    def test_view_attributes_and_multiline_text(self):
        self.assertEqual(self.values('''view! {
            <button aria-label="Open menu" title="Actions">
                "Click here"
            </button>
        }'''), ['Open menu', 'Actions', 'Click here'])

    def test_interpolated_and_raw_messages(self):
        self.assertEqual(self.values('''
            Err(format!("{name}: unsupported file type"));
            set_error.set(Some(format!(r#"Unable to read {name}"#)));
            el.set_text_content(Some("Untitled"));
        '''), ['{name}: unsupported file type', 'Unable to read {name}', 'Untitled'])

    def test_translation_keys_machine_values_and_comments(self):
        self.assertEqual(self.values('''
            // view! { <div>"Comment"</div> }
            /* nested /* comment */ "ignored" */
            view! { <div class="menu-item active">{t!("menu-open")}</div> }
            format!("width: {width}px");
            assert_eq!(x, "Fixture text");
            match key { "Escape" => close(), _ => () }
        '''), [])

    def test_production_after_test_module_is_checked(self):
        self.assertEqual(self.values('''
            #[cfg(test)] mod tests { fn example() { view! { <p>"Fixture"</p> } } }
            fn production() { view! { <p>"Translate me"</p> } }
        '''), ['Translate me'])

    def test_unknown_key_inputs_include_qualified_and_direct_calls(self):
        self.assertEqual(list(audit.referenced_keys('crate::t!("missing-key"); i18n::translate("known-key", None);')),
                         [(1, 'missing-key'), (1, 'known-key')])

    def test_match_on_t_is_not_a_translation(self):
        self.assertEqual(list(audit.referenced_keys('match t { "gt" => value, _ => other }')), [])

    def test_translated_attributes_are_not_literals(self):
        self.assertEqual(self.values('view! { <button title={t!("menu-open")}>{t!("menu-open")}</button> }'), [])


    def test_test_function_annotations_hide_fixtures_only(self):
        for annotation in ['test', 'wasm_bindgen_test', 'wasm_bindgen_test(async)',
                           'wasm_bindgen_test::wasm_bindgen_test']:
            with self.subTest(annotation=annotation):
                source = f'''#[{annotation}]
                    fn fixture() {{ view! {{ <p>"Fixture text"</p> }} translate("fixture-key", None); }}
                    fn production() {{ view! {{ <p>"Visible text"</p> }} translate("production-key", None); }}'''
                self.assertEqual(self.values(source), ['Visible text'])
                self.assertEqual([key for _, key in audit.referenced_keys(source)], ['production-key'])

    def test_cfg_requires_test_before_exempting_an_item(self):
        for condition in ['test', 'all(test, target_arch = "wasm32")',
                          'all(target_arch = "wasm32", all(feature = "browser", test))']:
            with self.subTest(condition=condition):
                source = f'''#[cfg({condition})]
                    mod fixtures {{ fn fixture() {{ view! {{ <p>"Fixture text"</p> }} translate("fixture-key", None); }} }}
                    fn production() {{ view! {{ <p>"Visible text"</p> }} translate("production-key", None); }}'''
                self.assertEqual(self.values(source), ['Visible text'])
                self.assertEqual([key for _, key in audit.referenced_keys(source)], ['production-key'])

    def test_production_cfg_mentions_of_test_remain_visible(self):
        for annotation in ['cfg(not(test))', 'cfg(any(test, target_arch = "wasm32"))',
                           'cfg(all(any(test, target_arch = "wasm32"), feature = "browser"))',
                           'cfg(feature = "test")', 'cfg_attr(test, test)']:
            with self.subTest(annotation=annotation):
                source = f'''#[{annotation}]
                    fn production() {{ view! {{ <p>"Visible text"</p> }} translate("production-key", None); }}'''
                self.assertEqual(self.values(source), ['Visible text'])
                self.assertEqual([key for _, key in audit.referenced_keys(source)], ['production-key'])


if __name__ == '__main__':
    unittest.main()
