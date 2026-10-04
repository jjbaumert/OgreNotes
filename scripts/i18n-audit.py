#!/usr/bin/env python3
"""Check user-facing Rust literals and locale coverage without external packages.

This is a source lint, not a Rust type checker. It recognizes view text and
accessible attributes, DOM text setters, error returns, UI signal setters, and
prose in format! calls. Exact exemptions document non-UI protocol text. Fluent
syntax and formatting are checked by the frontend's catalog tests.
"""
from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent


@dataclass
class Token:
    value: str
    start: int
    end: int
    string: bool = False


def tokenize(source: str) -> list[Token]:
    """Skip comments; keep Rust strings (including raw strings) as single tokens."""
    tokens = []
    i = 0
    while i < len(source):
        start = i
        if source[i].isspace():
            i += 1
            continue
        if source.startswith('//', i):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
            continue
        if source.startswith('/*', i):
            i += 2
            depth = 1
            while depth and i < len(source):
                if source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            continue
        raw = re.compile(r'(?:br|r)(#*)"').match(source, i)
        if raw:
            terminator = '"' + raw[1]
            begin = i + len(raw[0])
            end = source.find(terminator, begin)
            if end < 0:
                raise ValueError('unterminated raw string')
            i = end + len(terminator)
            tokens.append(Token(source[begin:end], start, i, True))
            continue
        if source[i] == '"':
            i += 1
            while i < len(source):
                if source[i] == '\\':
                    i += 2
                elif source[i] == '"':
                    break
                else:
                    i += 1
            value = source[start + 1:i]
            i += 1
            tokens.append(Token(value, start, i, True))
            continue
        char = re.compile(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'").match(source, i)
        if char:
            i += len(char[0])
            tokens.append(Token(char[0], start, i))
            continue
        word = re.compile(r'[A-Za-z_][A-Za-z_0-9]*').match(source, i)
        if word:
            i += len(word[0])
            tokens.append(Token(word[0], start, i))
        else:
            tokens.append(Token(source[i], start, i + 1))
            i += 1
    return tokens


def groups(tokens):
    ends, parents, stack = {}, {}, []
    for i, token in enumerate(tokens):
        parents[i] = tuple(stack)
        if token.string:
            continue
        if token.value in ('(', '[', '{'):
            stack.append(i)
        elif token.value in (')', ']', '}') and stack:
            ends[stack.pop()] = i
    return ends, parents


def call_name(tokens, opening):
    prev = opening - 1
    if prev >= 0 and tokens[prev].value == '!':
        prev -= 1
    return tokens[prev].value if prev >= 0 else ''


def translation_call(tokens, opening):
    return tokens[opening].value == '(' and (
        call_name(tokens, opening) == 'translate' or
        (call_name(tokens, opening) == 't' and tokens[opening - 1].value == '!')
    )


def cfg_requires_test(tokens):
    """Only explicit test predicates and conjunctions requiring them are safe."""
    if len(tokens) == 1:
        return not tokens[0].string and tokens[0].value == 'test'
    if len(tokens) < 3 or [t.value for t in tokens[:2]] != ['all', '('] or tokens[-1].value != ')':
        return False
    clauses, clause, depth = [], [], 0
    for token in tokens[2:-1]:
        if not token.string and token.value == ',' and depth == 0:
            clauses.append(clause)
            clause = []
            continue
        clause.append(token)
        if not token.string:
            if token.value == '(':
                depth += 1
            elif token.value == ')':
                depth -= 1
                if depth < 0:
                    return False
    if depth != 0:
        return False
    clauses.append(clause)
    return any(cfg_requires_test(clause) for clause in clauses)


def test_annotation(tokens):
    name = ''.join(t.value for t in tokens[:next(
        (i for i, token in enumerate(tokens) if token.value == '('), len(tokens)
    )])
    return name in ('test', 'wasm_bindgen_test') or name.endswith('::wasm_bindgen_test')


def test_items(tokens, ends):
    ranges = []
    for i in range(len(tokens) - 3):
        if tokens[i].value != '#' or tokens[i + 1].value != '[':
            continue
        end = ends.get(i + 1, i + 1)
        attr = tokens[i + 2:end]
        test_only_cfg = (
            len(attr) >= 4 and [t.value for t in attr[:2]] == ['cfg', '(']
            and attr[-1].value == ')' and cfg_requires_test(attr[2:-1])
        )
        test_function = test_annotation(attr)
        if not test_only_cfg and not test_function:
            continue
        # Bound the exemption to this annotated item, including any following
        # attributes, so production code after fixtures is still audited.
        cursor = end + 1
        while cursor < len(tokens) and (tokens[cursor].string or tokens[cursor].value not in ('{', ';')):
            cursor += 1
        if test_function and not any(t.value == 'fn' and not t.string for t in tokens[end + 1:cursor]):
            continue
        if cursor in ends:
            ranges.append((i, ends[cursor]))
    return ranges


# Stable product/protocol names and keycaps are intentionally untranslated.
UI_NAMES = {'OgreNotes', 'Mermaid', 'Esc'}

# These exact strings are protocol instructions or input examples, not chrome.
# Keep the reason beside each exemption so new UI text cannot inherit a broad
# file-level suppression.
EXEMPT_LITERALS = {
    ('frontend/src/components/at_menu.rs', ' Focus on: {}.'): 'AI prompt instruction',
    ('frontend/src/components/at_menu.rs', 'Summarize {scope_label} concisely.{topic_hint}'): 'AI prompt instruction',
    ('frontend/src/components/at_menu.rs', 'Translate {scope_label} to {target}. Preserve formatting where possible.'): 'AI prompt instruction',
    ('frontend/src/components/at_menu.rs', 'Rewrite {scope_label} to be {tone}. Return only the rewritten text.'): 'AI prompt instruction',
    ('frontend/src/components/at_menu.rs', r'Brainstorm 5 concise, distinct bullet points about: {topic}\n\nReturn only the bullet list.'): 'AI prompt instruction',
    ('frontend/src/components/kanban_card_modal.rs', 'bug|red;ux|blue'): 'Example of the accepted label/color syntax',
    ('frontend/src/components/profile_settings.rs', 'sk-ant-…'): 'API key prefix',
    ('frontend/src/pages/login.rs', r'\u{1F469} Alice'): 'Dev-login test identity',
    ('frontend/src/pages/login.rs', r'\u{1F468} Bob'): 'Dev-login test identity',
}


def prose(value):
    # Placeholders and Rust unicode escapes are not English words.
    text = re.sub(r'\\u\{[^}]*\}|\{[^}]*\}', '', value)
    return bool(re.search(r'[A-Za-z]{2,}', text))


def audit_source(source, path=''):
    tokens = tokenize(source)
    ends, parents = groups(tokens)
    tests = test_items(tokens, ends)
    view_ranges = [(i, ends[i]) for i in ends if call_name(tokens, i) == 'view']
    findings = []
    for i, token in enumerate(tokens):
        if not token.string or token.value in UI_NAMES or (path, token.value) in EXEMPT_LITERALS:
            continue
        if any(start <= i <= end for start, end in tests):
            continue
        ancestry = [call_name(tokens, p) for p in parents[i]]
        if any(translation_call(tokens, p) and i == p + 1 for p in parents[i]):
            continue
        # CSS and HTML constructors carry machine syntax, not UI prose. Text
        # embedded inside generated HTML is checked at its Rust text source.
        plain = re.sub(r'\{[^}]*\}', 'x', token.value).strip()
        machine_syntax = bool(re.match(r'https?://|<|(?:x[; ]+)?(?:background(?:-image|-color)?|color|height|width|top|left|right|bottom|position|text-align|padding-inline-start|transform|outline):', plain))
        css_class_list = bool(re.fullmatch(r'[a-z0-9_-]+(?: [a-z0-9_-]+)+', plain)) and '-' in plain
        if machine_syntax or css_class_list:
            continue
        if any(name in ('assert', 'assert_eq', 'assert_ne', 'debug_assert', 'panic',
                        'expect', 'println', 'eprintln', 'warn', 'error', 'info',
                        'debug', 'trace', 'warn_1', 'error_1', 'log_1', 'info_1') for name in ancestry):
            continue
        if not prose(token.value):
            continue
        before = ''.join(t.value for t in tokens[max(0, i - 8):i])
        in_view = any(start < i < end for start, end in view_ranges)
        attribute = in_view and bool(re.search(r'(?:aria-label|placeholder|title|alt|message|confirm_label)=$', before))
        previous = tokens[i - 1].value if i else ''
        following = tokens[i + 1].value if i + 1 < len(tokens) else ''
        text = in_view and ((previous == '>' and (i < 2 or tokens[i - 2].value != '=')) or (following == '<' and previous not in ('=', ':', '>')))
        dom = 'set_text_content' in ancestry
        ui_signal = bool(re.search(r'\s|^[A-Z]', token.value)) and any(call_name(tokens, p) == 'set' and p >= 3 and tokens[p - 3].value.startswith('set_') for p in parents[i])
        error = any(name in ('Err', 'map_err', 'ok_or_else') for name in ancestry)
        presentation = not path or '/components/' in path or '/pages/' in path
        error = error and presentation
        formatted = presentation and 'format' in ancestry and bool(re.search(r'[A-Za-z][^\n]*\s', token.value))
        if attribute or text or dom or ui_signal or error or formatted:
            kind = 'attribute' if attribute else 'text' if text or dom else 'message'
            findings.append((source.count('\n', 0, token.start) + 1, kind, token.value))
    return findings


def referenced_keys(source):
    tokens = tokenize(source)
    ends, parents = groups(tokens)
    tests = test_items(tokens, ends)
    for i, token in enumerate(tokens):
        if not token.string or any(start <= i <= end for start, end in tests):
            continue
        if any(translation_call(tokens, p) and i == p + 1 for p in parents[i]):
            yield source.count('\n', 0, token.start) + 1, token.value


def catalog_keys(path):
    keys = re.findall(r'^([a-z][a-z0-9-]*)\s*=', path.read_text(), re.M)
    duplicates = {key for key, count in Counter(keys).items() if count > 1}
    return set(keys), duplicates


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inventory', action='store_true', help='report without failing on source literals')
    args = parser.parse_args()
    errors = []
    baseline, _ = catalog_keys(ROOT / 'frontend/locales/en-US/main.ftl')
    for path in sorted((ROOT / 'frontend/locales').glob('*/main.ftl')):
        keys, duplicates = catalog_keys(path)
        for issue, values in [('missing', baseline - keys), ('extra', keys - baseline), ('duplicate', duplicates)]:
            if values:
                errors.append(f'{path.relative_to(ROOT)}: {issue} keys: {", ".join(sorted(values))}')
    for path in sorted((ROOT / 'frontend/src').rglob('*.rs')):
        if 'tests' in path.parts or path.name in ('eval.rs', 'parser.rs', 'functions.rs'):
            continue
        source = path.read_text()
        for line, key in referenced_keys(source):
            if key not in baseline:
                errors.append(f'{path.relative_to(ROOT)}:{line}: unknown translation key: {key}')
        for line, kind, value in audit_source(source, str(path.relative_to(ROOT))):
            errors.append(f'{path.relative_to(ROOT)}:{line}:{kind}: {value}')
    for error in errors:
        print(error)
    print(f'i18n-audit: {len(errors)} findings')
    return 0 if args.inventory or not errors else 1


if __name__ == '__main__':
    sys.exit(main())
