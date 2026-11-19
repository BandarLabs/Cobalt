"""Extract per-screen state surfaces for APPQA-02 (differentiate loading,
empty, offline, expired credentials and malformed content).

For every ScreenBuilder::new("id") in an app's src/, attribute the state
constructors that follow inside the same fn body to that screen id:
activity/skeleton (loading), empty_state/splash (empty), error_state/banner
(failure) - and capture the message strings so a human can judge whether
offline, expired-credential and malformed-content failures read differently.
Also collects detection-side signals per app (offline/401/expired/parse
keywords outside screens). Extraction only; verdicts are human.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent

SCREEN_NEW = re.compile(r'ScreenBuilder::new\(\s*"([^"]+)"')
STATE_CALLS = re.compile(
    r'\.(activity|skeleton|empty_state|error_state|splash|banner)\s*\(')
STRING_LIT = re.compile(r'"((?:[^"\\]|\\.)*)"')
SIGNALS = re.compile(
    r'(?i)(offline|expired|expire|unauthori[sz]ed|401|403|credentials?|'
    r'token|malformed|could not (parse|read|open)|invalid|network|'
    r'no connection|unreachable)')

sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module
dom = import_module('audit-dominant-actions')


def extract_app(src_dir):
    screens = {}
    signals = set()
    for rs in sorted(src_dir.rglob('*.rs')):
        source = dom.strip_test_modules(rs.read_text())
        for m in SIGNALS.finditer(source):
            signals.add(m.group(1).lower())
        for fname, body in dom.split_functions(source):
            pos = 0
            current = None
            for m in re.finditer(r'ScreenBuilder::new\(\s*"([^"]+)"|\.(activity|skeleton|empty_state|error_state|splash|banner)\s*\(', body):
                if m.group(1):
                    current = m.group(1)
                    screens.setdefault(current, [])
                elif current and m.group(2):
                    args = dom.balanced_args(body, m.end() - 1)
                    lits = [s for s in STRING_LIT.findall(args) if len(s) > 2]
                    screens[current].append({'kind': m.group(2), 'text': lits[:3]})
    return screens, sorted(signals)


def main():
    out = {}
    app_dirs = sorted((ROOT / 'apps').iterdir()) + sorted((ROOT / 'examples').iterdir())
    for d in app_dirs:
        if not (d / 'cobalt-app.json').exists() or not (d / 'src').is_dir():
            continue
        screens, signals = extract_app(d / 'src')
        if screens:
            out[d.name] = {'screens': screens, 'signals': signals}
    dest = ROOT / 'docs/quality/evidence/appqa02/states.json'
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + '\n')
    print(len(out), 'apps ->', dest)


if __name__ == '__main__':
    main()
