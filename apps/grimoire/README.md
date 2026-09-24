# Grimoire

An offline reference and table tracker for fifth-edition tabletop games,
built from the SRD.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/initiative.png" alt="Initiative order for a table of six"><br>Initiative order for a table of six</td>
<td width="50%" valign="top"><img width="300" src="screenshots/rule.png" alt="A rule section"><br>A rule section</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/magic-items.png" alt="Magic items by category and rarity"><br>Magic items by category and rarity</td>
<td width="50%" valign="top"><img width="300" src="screenshots/party-member.png" alt="A party member's health and death saves"><br>A party member's health and death saves</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/spell-filters.png" alt="Spells filtered by class, level and school"><br>Spells filtered by class, level and school</td>
</tr>
</table>

## Features

- 1,349 SRD records: spells, monsters, conditions, rules, rule sections and
  magic items. **About** shows exactly what the build contains.
- Prefix search, bookmarks and an edition switch between the 2014 and 2024
  SRD.
- Filter spells by class, level, school, ritual and concentration. Filter
  monsters by challenge rating and type.
- Stat blocks and rules open in the document reader, with abilities and
  headings intact. Tables are written out row by row so they stay readable at
  large text sizes.
- **Initiative**: add combatants by name or from the monster reference. It
  sorts the order, tracks the active turn and round, and **Previous** takes
  back a turn passed by mistake. Tap a combatant to hand them the turn, fix
  their roll or remove them.
- **Party**: up to six members, each with health, death saves and nine spell
  slot levels.
- Initiative, party and bookmarks are saved after every change.

## Limits

The 2024 SRD snapshot has conditions, monsters and magic items only. Its
spells and rule sections are not published by the 5e-bits source, so they are
left out rather than substituted. See [data/SOURCES.md](data/SOURCES.md).

## Building the index

`tools/build_corpus.py` builds the index from the reviewed snapshots in
`data/source/`. It does not download anything.

## Permissions

None. Grimoire runs offline.

## Development

```sh
cargo test -p kobo-grimoire
python3 scripts/check-apps-sim.py grimoire
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`.

## Credits

This work includes material taken from the System Reference Document 5.1 and System Reference Document 5.2 by Wizards of the Coast LLC, available under the Creative Commons Attribution 4.0 International License.

Grimoire is unofficial and not endorsed by Wizards of the Coast. See [THIRD-PARTY.md](THIRD-PARTY.md).
