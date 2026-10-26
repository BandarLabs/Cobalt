#!/usr/bin/env python3
"""Drive Grimoire through a table session in the actual simulator: look a
reference up, read a long one to its end, run initiative for six, and reopen
everything after a restart."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
STATE = 'grimoire-state-v2'
PARTY = [('Aria', 15, 24), ('Brannoc', 16, 30), ('Cass', 13, 21),
         ('Delphine', 14, 26), ('Emeric', 17, 33), ('Fenn', 12, 19)]
COMBATANTS = [('Aria', 18), ('Brannoc', 15), ('Cass', 12), ('Delphine', 9), ('Emeric', 6)]


def saved_state():
    initiative = ';'.join(f'{name}~{roll}' for name, roll in COMBATANTS)
    party = ';'.join(f'{name}~{ac}~{hp}~{hp}~0~0~000000000' for name, ac, hp in PARTY)
    return f'2014|1|0|{initiative}|{party}|'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target'))).resolve()
    cli = target/'debug/kobo'
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-grimoire-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='srd-corpus', KOBO_SIM_SEED='0')
        state = private/'cobalt-sim-state/grimoire'
        state.mkdir(parents=True)
        (state/STATE).write_text(saved_state())
        log_path = output/'simulator.log'
        checks = []
        with log_path.open('w') as log:
            def stop():
                nonlocal process
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                process = None

            def start():
                nonlocal process
                log.seek(0)
                log.truncate()
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'apps/grimoire', env=env, stdout=log,
                                           stderr=log, start_new_session=True)
                deadline = time.monotonic()+120
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError(log_path.read_text()[-3000:])
                    found = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                      log_path.read_text())
                    if found:
                        return found.group(1)
                    time.sleep(.1)
                raise RuntimeError('Simulator did not start')

            def drive(*steps):
                command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=120)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                assert not [i for i in diagnostics['issues'] if i['severity'] == 'error'], diagnostics
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'grimoire', metadata
                return layout

            def says(layout, text):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return text in ' '.join(drawn.split())

            def page_forward():
                """Turns to the next page of a list, if there is one."""
                layout = get('layout')
                at = position(layout)
                if not at or at[0] >= at[1]:
                    return False
                drive('tap-id next', 'wait-idle')
                return True

            def reach(title):
                """Finds a row wherever it has been paginated to.

                At the larger text sizes a party of six is two pages, and a
                journey that only ever looked at the first one would be
                testing the panel rather than the application. Starts from the
                first page, because the list may have been left on a later one.
                """
                for _ in range(8):
                    at = position(get('layout'))
                    if not at or at[0] == 1:
                        break
                    drive('tap-id previous', 'wait-idle')
                for _ in range(8):
                    if says(get('layout'), title):
                        return
                    assert page_forward(), f'no page of this list says {title!r}'
                raise AssertionError(f'{title!r} was not reached')

            def position(layout):
                for node in layout['nodes']:
                    if 'Position' in node['kind']:
                        for line in node['lines']:
                            found = re.search(r'(\d+) of (\d+)', str(line))
                            if found:
                                return int(found.group(1)), int(found.group(2))
                return None

            try:
                address = start()
                drive('wait-for-id spells')
                home = capture('01-home')
                for tile in ['Spells', 'Monsters', 'Rules & conditions', 'Magic items',
                             'Dice', 'Initiative', 'Party']:
                    assert says(home, tile), f'{tile} is not on the home screen'
                checks.append('every category the reference holds has a way in')

                # The magic items this build ships, which had no way in before.
                drive('tap-id items')
                items = capture('02-magic-items')
                assert says(items, 'Adamantine Armor'), items
                assert says(items, 'Armor · Uncommon'), 'the item subtitle is still a field name'
                assert not says(items, 'Filters'), 'a category with no filters offered the word'
                checks.append('bundled magic items are listed with their category and rarity')

                reach('Adamantine Armor')
                drive('tap Adamantine Armor')
                item = capture('03-item-reference')
                assert says(item, 'reinforced with adamantine'), item

                # A long rule, read to its last page.
                drive('tap Back', 'tap Back', 'tap-id rules', 'tap-id search')
                drive('type traps', 'tap-id kb.enter')
                found = capture('04-rule-search')
                reach('Traps')
                drive('tap Traps')
                first = capture('05-long-rule-first-page')
                page, total = position(first)
                assert (page, total) == (1, total) and total > 1, f'{page} of {total}'
                for _ in range(total-1):
                    drive('tap-id reader-forward')
                last = capture('06-long-rule-last-page')
                assert position(last) == (total, total), position(last)
                checks.append(f'a {total}-page rule is reachable to its last page')

                # A category this edition does not hold explains itself.
                drive('tap Back', 'tap Back', 'tap-id spells', 'tap-id edition-2024')
                missing = capture('07-edition-without-spells')
                assert says(missing, 'No spells in the 2024 reference'), missing
                assert says(missing, '2014'), missing
                checks.append('an empty category says why rather than blaming a filter')

                # A 2024 condition, which used to open with nothing under it.
                # The edition stays where it was left, which is the point.
                drive('tap Back', 'tap-id rules')
                condition = capture('08-2024-conditions')
                reach('Blinded')
                drive('tap Blinded')
                blinded = capture('09-2024-condition-text')
                assert says(blinded, 'Blinded condition'), blinded
                checks.append('2024 conditions carry their text')

                # Initiative for a table of six.
                drive('tap Back', 'tap Back', 'tap-id initiative')
                order = capture('10-initiative')
                assert says(order, 'Round 1 · turn 1 of 5'), order
                # The initiative roll needs the keyboard's other layer, the way
                # it does for anyone entering a number on this device.
                drive('tap-id init-add', 'type goblin', 'tap-id kb.enter',
                      'tap-id kb.layer', 'type 7', 'tap-id kb.enter')
                six = capture('11-six-combatants')
                assert says(six, 'Round 1 · turn 1 of 6'), six
                reach('goblin')
                drive('tap-id turn-next', 'tap-id turn-next')
                third = capture('12-third-turn')
                assert says(third, 'turn 3 of 6'), third
                drive('tap-id turn-previous')
                assert says(get('layout'), 'turn 2 of 6')
                checks.append('a turn passed by accident is taken back')

                reach('Cass')
                drive('tap Cass')
                chosen = capture('13-combatant')
                assert says(chosen, 'Cass'), chosen
                drive('tap-id init-take')
                taken = capture('14-turn-taken-from-a-row')
                assert says(taken, 'turn 3 of 6'), taken

                # A party member takes a hit before the session is put down.
                drive('tap Back', 'tap-id party')
                capture('15-party')
                reach('Delphine')
                drive('tap Delphine', 'tap-id hp-down', 'tap-id hp-down', 'tap-id save-failure')
                member = capture('16-party-member')
                assert says(member, 'HP 24/26'), member
                assert says(member, '0 success · 1 failure'), member
                # The slots are the second page of a member, where the end of a
                # long rest goes looking for them.
                drive('tap-id next', 'tap-id slot-2')
                slots = capture('17-party-member-slots')
                assert says(slots, 'Spell slots · 1 used'), slots

                stop()
                address = start()
                drive('wait-for-id initiative', 'tap-id initiative')
                reopened = capture('18-reopened-initiative')
                assert says(reopened, 'turn 3 of 6'), reopened
                reach('goblin')
                drive('tap Back', 'tap-id party')
                reach('Delphine')
                drive('tap Delphine')
                kept = capture('19-reopened-member')
                assert says(kept, 'HP 24/26'), kept
                assert says(kept, '0 success · 1 failure'), kept
                checks.append('the table state of an evening survives a restart')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale, 'fixture': 'srd-corpus',
                    'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
