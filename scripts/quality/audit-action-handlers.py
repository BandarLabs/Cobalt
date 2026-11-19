"""List every action id an app's screens can show, and whether on_action handles it.

Static first pass for APPQA-05 ("audit every visible action for a handler").
Registered: ids passed to screen-building calls (button, buttons, row, tile,
page_turns, reading_menu, hold, modal buttons, keyboard submits, tabs).
Handled: ids compared or matched inside fn on_action, including ids forwarded
to sub-component act()/on_action helpers that themselves compare action_id.
Candidates are registered ids with no visible handler - each needs a human
look (SDK-dispatched ids and delegated handlers can false-positive).
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent

BUILDER_CALLS = re.compile(
    r'(?:button|buttons|one_line_row|clamped_row|row_with_menu|rows_with_menu|'
    r'tile|tiles|page_turns|reading_menu|hold|tab|tabs|modal|facts|banner|'
    r'submit|keyboard|nav_bar|with_menu)\s*\(')
CONST_DEF = re.compile(r'const\s+([A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*"([^"]+)"')
ACTION_ID = re.compile(r'action_id\(\s*(?:&)?(?:format!\([^)]*\)|"([^"]+)"|([A-Z][A-Z0-9_]*))\s*\)')
STRING_LIT = re.compile(r'"([a-z][a-z0-9-]{1,32})"')
ON_ACTION = re.compile(r'fn\s+(on_action|act)\s*\(')


def split_functions(source):
    """Rough top-level fn bodies: (name, text) pairs by brace counting."""
    out = []
    for m in re.finditer(r'fn\s+([a-z_]+)\s*\(', source):
        start = source.index('{', m.end())
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


def strip_test_modules(source):
    """Remove every #[cfg(test)] item, skipping braces string-aware.

    Test modules are interleaved through these files, so a naive split at the
    first attribute throws away real handlers below it.
    """
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
            n = source[k + 1] if k + 1 < len(source) else ''
            if in_line:
                if c == '\n':
                    in_line = False
            elif in_block:
                if c == '*' and n == '/':
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
            elif c == '/' and n == '/':
                in_line = True
            elif c == '/' and n == '*':
                in_block = True
            elif c == '"':
                in_str = True
            elif c == "'" and (n.isalnum() or n == '\\') and source[k + 2:k + 3] in ("'", ''):
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


def audit(main_rs):
    source = main_rs.read_text()
    consts = dict(CONST_DEF.findall(source))
    # action_id() outside tests is a handler comparison; builder calls take
    # the bare constant or literal. TextEntry::opened_by(X) routes X through
    # the SDK entry field, which counts as handled.
    body = strip_test_modules(source)
    handled = set()
    for m in ACTION_ID.finditer(body):
        handled.add(m.group(1) or consts.get(m.group(2) or '', m.group(2)))
    for m in re.finditer(r'opened_by\(\s*(?:"([^"]+)"|([A-Z][A-Z0-9_]*))\s*\)', body):
        handled.add(m.group(1) or consts.get(m.group(2) or '', m.group(2)))
    # Page-turn ids are dispatched to fn on_page_turn, not on_action.
    if 'fn on_page_turn' in body:
        for m in re.finditer(r'page_turns\(\s*"([^"]+)"\s*,\s*"([^"]+)"', body):
            handled.update((m.group(1), m.group(2)))
    # The is("...") idiom: a closure comparing action_id(name) at runtime.
    if re.search(r'action_id\([a-z_]+\)', body):
        for m in re.finditer(r'\bis\("([^"]+)"\)', body):
            handled.add(m.group(1))
    forwarded = set()
    for m in re.finditer(r'self\.([a-z_]+)\.(?:act|on_action|handle)\(', body):
        forwarded.add(m.group(1))
    registered = set()
    lines = body.splitlines()
    for i, line in enumerate(lines):
        if not BUILDER_CALLS.search(line):
            continue
        window = '\n'.join(lines[i:i + 6])
        for name, value in consts.items():
            if re.search(rf'\b{name}\b', window):
                registered.add(value)
        for lit in STRING_LIT.findall(window):
            registered.add(lit)
    # format!-generated ids (book-{index} etc.) cannot be enumerated statically.
    dynamic = bool(re.search(r'action_id\(\s*&?format!', source))
    candidates = sorted(
        a for a in registered - handled
        if a not in ('back',) and not a.startswith('{') and ' ' not in a
    )
    return {
        'registered': sorted(registered),
        'handled': sorted(h for h in handled if h),
        'delegated_fields': sorted(forwarded),
        'dynamic_ids': dynamic,
        'candidates': candidates,
    }


def main():
    report = {}
    for main_rs in sorted(ROOT.glob('apps/*/src/main.rs')) + sorted(ROOT.glob('examples/*/src/main.rs')):
        app = main_rs.parts[-3]
        if app == 'zotero-reader':
            continue  # APPQA-13
        report[app] = audit(main_rs)
    out = ROOT / 'docs/quality/evidence/appqa05/action-handlers.json'
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, indent=1) + '\n')
    total = sum(len(r['candidates']) for r in report.values())
    print(f'{len(report)} apps audited; {total} candidate ids; report: {out.relative_to(ROOT)}')
    for app, r in report.items():
        if r['candidates']:
            print(f"  {app}: {', '.join(r['candidates'])}")


if __name__ == '__main__':
    sys.exit(main())
