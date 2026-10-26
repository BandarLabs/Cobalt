"""Extract per-screen control layouts for APPQA-01 (one dominant task action).

For every ScreenBuilder::new("id") in an app's src/, attribute the builder
method calls that follow inside the same fn body (including `screen = screen.`
reassignment chains) to that screen id. Output: per app, per screen, the
controls it can show - primary buttons, plain buttons, action bar entries,
rows/tiles (row taps are the dominant action of a list screen), tabs and
modals. Verdicts are a human pass over this table; the script only extracts.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent

SCREEN_NEW = re.compile(r'ScreenBuilder::new\(\s*"([^"]+)"')
CALLS = re.compile(
    r'\.(primary_button(?:_with_state)?|button(?:_with_state)?|buttons|'
    r'action_bar(?:_marked)?|one_line_row|clamped_row|row_with_menu|'
    r'rows_with_menu|tiles?|page_turns|reading_menu|hold|modal|tabs?|submit|'
    r'facts|banner|activity|splash)\s*\(')
STRING_LIT = re.compile(r'"((?:[^"\\]|\\.)*)"')


def strip_test_modules(source):
    out = []
    i = 0
    for m in re.finditer(r'#\[cfg\(test\)\]', source):
        j = source.find('{', m.end())
        if j == -1 or j > m.end() + 200:
            continue
        out.append(source[i:m.start()])
        depth = 0
        k = j
        in_str = in_char = in_line = in_block = False
        while k < len(source):
            c = source[k]
            nxt = source[k:k + 2]
            if in_line:
                if c == '\n':
                    in_line = False
            elif in_block:
                if nxt == '*/':
                    in_block = False
                    k += 1
            elif in_str:
                if c == '\\':
                    k += 1
                elif c == '"':
                    in_str = False
            elif in_char:
                if c == '\\':
                    k += 1
                elif c == "'":
                    in_char = False
            elif nxt == '//':
                in_line = True
                k += 1
            elif nxt == '/*':
                in_block = True
                k += 1
            elif c == '"':
                in_str = True
            elif c == "'":
                in_char = True
            elif c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    k += 1
                    break
            k += 1
        i = k
    out.append(source[i:])
    return ''.join(out)


def split_functions(source):
    out = []
    for m in re.finditer(r'fn\s+([a-z_]+)\s*\(', source):
        start = source.find('{', m.end())
        if start == -1:
            continue
        depth = 0
        for i in range(start, len(source)):
            if source[i] == '{':
                depth += 1
            elif source[i] == '}':
                depth -= 1
                if depth == 0:
                    out.append((m.group(1), source[start:i + 1]))
                    break
    return out


def balanced_args(text, open_paren):
    depth = 0
    in_str = in_char = False
    for i in range(open_paren, len(text)):
        c = text[i]
        if in_str:
            if c == '\\':
                i += 1
            elif c == '"':
                in_str = False
        elif in_char:
            if c == '\\':
                i += 1
            elif c == "'":
                in_char = False
        elif c == '"':
            in_str = True
        elif c == "'":
            in_char = True
        elif c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                return text[open_paren + 1:i]
    return text[open_paren + 1:]


def label_of(method, args):
    """Best-effort visible label(s) for a control call."""
    lits = STRING_LIT.findall(args)
    if method.startswith('primary_button') or method.startswith('button'):
        # (name, label[, state]) - label is the second literal
        return lits[1] if len(lits) > 1 else (lits[0] if lits else '?')
    if method in ('buttons', 'action_bar', 'action_bar_marked'):
        # [(name, label), ...] - take every second literal
        return [l for i, l in enumerate(lits) if i % 2 == 1] or lits
    if method in ('tabs',):
        return lits
    return lits[:2]


def audit_app(app_dir):
    screens = {}
    for src in sorted((app_dir / 'src').rglob('*.rs')):
        text = strip_test_modules(src.read_text())
        for fn_name, body in split_functions(text):
            current = None
            for m in re.finditer(r'ScreenBuilder::new\(\s*"([^"]+)"\)|\.('
                                 r'primary_button(?:_with_state)?|button(?:_with_state)?|buttons|'
                                 r'action_bar(?:_marked)?|one_line_row|clamped_row|row_with_menu|'
                                 r'rows_with_menu|tiles?|page_turns|reading_menu|hold|modal|tabs?|submit|'
                                 r'facts|banner|activity|splash)\s*\(', body):
                if m.group(1):
                    current = m.group(1)
                    screens.setdefault(current, {'fn': fn_name,
                                                 'file': str(src.relative_to(app_dir)),
                                                 'controls': []})
                elif current and m.group(2):
                    method = m.group(2)
                    open_paren = body.index('(', m.end() - 1)
                    args = balanced_args(body, open_paren)
                    screens[current]['controls'].append(
                        {'method': method, 'labels': label_of(method, args)})
    return screens


def main():
    out = {}
    for base in (ROOT / 'apps', ROOT / 'examples'):
        for d in sorted(base.iterdir()):
            if not (d / 'cobalt-app.json').exists():
                continue
            if d.name in ('zotero-reader',):
                continue
            screens = audit_app(d)
            if screens:
                out[d.name] = screens
    dest = ROOT / 'docs/quality/evidence/appqa01/screens.json'
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + '\n')
    n_screens = sum(len(v) for v in out.values())
    print(f'{len(out)} apps, {n_screens} screens -> {dest.relative_to(ROOT)}')


if __name__ == '__main__':
    sys.exit(main())
