//! Builds "First Light", the tutorial story bundled with the app, as a
//! Z-machine version 3 file.
//!
//! No Z-machine compiler exists in this repository, so the story is
//! assembled by hand from the pieces the interpreter in `zvm` already
//! understands: a header, a globals table, input buffers, a dictionary, an
//! empty object table, and one flat routine. One routine is enough: the
//! whole game is a read loop with jumps and branches, state kept in globals,
//! so no call frames or packed routine addresses are needed. Version 3
//! because its dictionary words and memory layout are the smallest, and the
//! interpreter's v3 paths are already proven end to end by the Zork I
//! fixture.

use std::collections::HashMap;

/// Address of the globals table; globals 0.. live here as words.
const GLOBALS: u16 = 0x40;
/// Keyboard buffer for the typed line (byte 0 is its capacity).
const TEXT_BUF: u16 = 0x220;
/// Tokenisation buffer the read opcode fills.
const PARSE_BUF: u16 = 0x280;
/// Where the dictionary sits. Everything before it is fixed-size, so the
/// base is a constant and word addresses are known before code is emitted.
const DICT: u16 = 0x2c0;

// Global variable numbers (variable 16 is the first global).
const G_ROOM: u8 = 16;
const G_LAMP: u8 = 17;
const G_MATCH: u8 = 18;
const G_LIT: u8 = 19;
const G_TURNS: u8 = 20;
const G_TMP: u8 = 21;
const G_TMP2: u8 = 22;

/// One assembler operand.
#[derive(Clone, Copy)]
enum Op {
    Large(u16),
    Small(u8),
    Var(u8),
}

impl Op {
    fn kind(self) -> u8 {
        match self {
            Op::Large(_) => 0,
            Op::Small(_) => 1,
            Op::Var(_) => 2,
        }
    }
    fn emit(self, out: &mut Vec<u8>) {
        match self {
            Op::Large(value) => out.extend_from_slice(&value.to_be_bytes()),
            Op::Small(value) | Op::Var(value) => out.push(value),
        }
    }
}

/// What a patch in the fixup list means. Both kinds hold a signed offset
/// relative to the address just after the instruction, minus 2, which is the
/// convention the interpreter applies to jumps and branches alike.
enum Fixup {
    /// 1OP 12: a two-byte large operand at the position.
    Jump(usize),
    /// Two branch bytes at the position; signed 14-bit, on-true, long form.
    Branch(usize),
}

struct Asm {
    bytes: Vec<u8>,
    labels: HashMap<&'static str, usize>,
    fixups: Vec<(Fixup, &'static str)>,
}

impl Asm {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            labels: HashMap::new(),
            fixups: Vec::new(),
        }
    }
    fn mark(&mut self, name: &'static str) {
        self.labels.insert(name, self.bytes.len());
    }
    fn op0(&mut self, n: u8) {
        self.bytes.push(0xb0 | n);
    }
    /// print (0OP 2) with an inline string.
    fn print(&mut self, text: &str) {
        self.op0(2);
        self.bytes.extend_from_slice(&encode_zstring(text));
    }
    fn quit(&mut self) {
        self.op0(10);
    }
    fn new_line(&mut self) {
        self.op0(11);
    }
    /// jump (1OP 12), always the two-byte large-operand form.
    fn jump(&mut self, label: &'static str) {
        self.bytes.push(0x8c); // 1OP 12, large operand
        self.fixups.push((Fixup::Jump(self.bytes.len()), label));
        self.bytes.extend_from_slice(&[0, 0]);
    }
    fn branch_to(&mut self, label: &'static str) {
        self.fixups.push((Fixup::Branch(self.bytes.len()), label));
        self.bytes.extend_from_slice(&[0, 0]);
    }
    /// je (2OP 1, VAR form) with two to four operands; branches to label
    /// when any pair is equal.
    fn je(&mut self, operands: &[Op], label: &'static str) {
        debug_assert!((2..=4).contains(&operands.len()));
        self.bytes.push(0xc1);
        let mut types = 0xffu8; // unused slots read as omitted
        for (index, operand) in operands.iter().enumerate() {
            types &= !(0b11 << (6 - index * 2));
            types |= operand.kind() << (6 - index * 2);
        }
        self.bytes.push(types);
        for operand in operands {
            operand.emit(&mut self.bytes);
        }
        self.branch_to(label);
    }
    /// jz (1OP 0) on a variable operand.
    fn jz_var(&mut self, variable: u8, label: &'static str) {
        self.bytes.push(0xa0);
        self.bytes.push(variable);
        self.branch_to(label);
    }
    /// jg (2OP 3, VAR form): branches when the variable beats the constant.
    fn jg_var_small(&mut self, variable: u8, value: u8, label: &'static str) {
        self.bytes.push(0xc3);
        self.bytes.push(0x9f); // variable, small
        self.bytes.push(variable);
        self.bytes.push(value);
        self.branch_to(label);
    }
    /// store (2OP 13, VAR form): store variable, value - no result byte.
    fn store(&mut self, variable: u8, value: u8) {
        self.bytes.push(0xcd);
        self.bytes.push(0x5f); // small, small
        self.bytes.push(variable);
        self.bytes.push(value);
    }
    /// inc (1OP 5) on a variable operand.
    fn inc(&mut self, variable: u8) {
        self.bytes.push(0xa5);
        self.bytes.push(variable);
    }
    /// loadw (2OP 15, VAR form) array, word index -> variable.
    fn loadw(&mut self, array: u16, index: u8, into: u8) {
        self.bytes.push(0xcf);
        self.bytes.push(0x1f); // large, small
        self.bytes.extend_from_slice(&array.to_be_bytes());
        self.bytes.push(index);
        self.bytes.push(into);
    }
    /// loadb (2OP 16, VAR form) array, byte index -> variable.
    fn loadb(&mut self, array: u16, index: u8, into: u8) {
        self.bytes.push(0xd0);
        self.bytes.push(0x1f);
        self.bytes.extend_from_slice(&array.to_be_bytes());
        self.bytes.push(index);
        self.bytes.push(into);
    }
    /// sread (VAR 4) with two large buffer addresses, the version 3 form.
    fn sread(&mut self) {
        self.bytes.push(0xe4);
        self.bytes.push(0x0f); // large, large
        self.bytes.extend_from_slice(&TEXT_BUF.to_be_bytes());
        self.bytes.extend_from_slice(&PARSE_BUF.to_be_bytes());
    }
    /// save (0OP 5) / restore (0OP 6): the app captures the suspension and
    /// the branch resolves taken on completion, refused otherwise.
    fn save(&mut self, done: &'static str) {
        self.op0(5);
        self.branch_to(done);
    }
    fn restore(&mut self, done: &'static str) {
        self.op0(6);
        self.branch_to(done);
    }
    fn link(mut self) -> Vec<u8> {
        for (fixup, label) in &self.fixups {
            let target = *self
                .labels
                .get(label)
                .unwrap_or_else(|| panic!("missing label {label}"));
            let (at, wide) = match fixup {
                Fixup::Jump(at) => (*at, true),
                Fixup::Branch(at) => (*at, false),
            };
            let offset = i64::try_from(target).expect("small code")
                - i64::try_from(at + 2).expect("small code")
                + 2;
            let offset = i16::try_from(offset).expect("jump in range");
            if wide {
                self.bytes[at..at + 2].copy_from_slice(&offset.to_be_bytes());
            } else {
                assert!((-8192..=8191).contains(&offset), "branch in range");
                let raw = u16::from_be_bytes(offset.to_be_bytes()) & 0x3fff;
                self.bytes[at] = 0x80 | u8::try_from(raw >> 8).expect("high bits fit");
                self.bytes[at + 1] = u8::try_from(raw & 0xff).expect("low bits fit");
            }
        }
        self.bytes
    }
}

/// Encodes ASCII text as Z-characters, terminated by the top bit on the
/// final word. Lowercase lives in alphabet 0, capitals shift to alphabet 1,
/// and the punctuation row shifts to alphabet 2; anything else goes out as
/// a ten-bit ZSCII escape.
fn encode_zstring(text: &str) -> Vec<u8> {
    const A2: &[u8; 26] = b" \n0123456789.,!?_#'\"/\\-:()";
    let mut zchars = Vec::new();
    for byte in text.bytes() {
        match byte {
            b'a'..=b'z' => zchars.push(byte - b'a' + 6),
            b'A'..=b'Z' => zchars.extend([4, byte - b'A' + 6]),
            b' ' => zchars.push(0),
            other => {
                if let Some(index) = A2.iter().position(|&c| c == other) {
                    zchars.extend([5, u8::try_from(index).expect("alphabet row") + 6]);
                } else {
                    zchars.extend([5, 6, other >> 5, other & 0x1f]);
                }
            }
        }
    }
    while zchars.len() % 3 != 0 {
        zchars.push(5);
    }
    let mut encoded = Vec::new();
    let groups = zchars.len() / 3;
    for (index, group) in zchars.chunks(3).enumerate() {
        let mut word =
            (u16::from(group[0]) << 10) | (u16::from(group[1]) << 5) | u16::from(group[2]);
        if index + 1 == groups {
            word |= 0x8000;
        }
        encoded.extend_from_slice(&word.to_be_bytes());
    }
    encoded
}

/// The dictionary words the story understands. The address of a word's entry
/// is what the read loop compares against, so order here is only storage.
const WORDS: &[&str] = &[
    "look",
    "examine",
    "read",
    "take",
    "get",
    "drop",
    "inventory",
    "i",
    "light",
    "north",
    "n",
    "south",
    "s",
    "quit",
    "note",
    "lamp",
    "match",
    "matches",
    "matchbox",
    "save",
    "restore",
];

/// v3 dictionary words are six Z-characters in two words, top bit on the
/// second.
fn encode_word(word: &str) -> [u8; 4] {
    let mut zchars: Vec<u8> = word.bytes().take(6).map(|b| b - b'a' + 6).collect();
    zchars.resize(6, 5);
    let first = (u16::from(zchars[0]) << 10) | (u16::from(zchars[1]) << 5) | u16::from(zchars[2]);
    let second =
        (u16::from(zchars[3]) << 10) | (u16::from(zchars[4]) << 5) | u16::from(zchars[5]) | 0x8000;
    let mut out = [0; 4];
    out[0..2].copy_from_slice(&first.to_be_bytes());
    out[2..4].copy_from_slice(&second.to_be_bytes());
    out
}

fn word(word: &str) -> Op {
    let index = WORDS.iter().position(|&w| w == word).expect("known word");
    // Dictionary header: separator count byte, entry length byte, count word.
    Op::Large(DICT + 4 + u16::try_from(index).expect("few words") * 4)
}

const INTRO: &str = "First Light\nAn interactive tutorial\n\nThe ferryman said the light had never once gone out on a keeper's first night, and then he laughed and rowed away.\n\n";
const ROOM_GATE: &str = "Gatehouse\nRain ticks on the shutters. A note is pinned to the door. The only path runs north.\n";
const ROOM_STORE: &str = "Storeroom\nShelves of oilskin and rope lean in the gloom.\n";
const LAMP_HERE: &str = "A brass lamp sits on the lowest shelf.\n";
const MATCH_HERE: &str = "A matchbox lies in the dust beside it.\n";
const ROOM_STAIR_DARK: &str = "Stair Head\nThe stair ends at a cold platform above the sea. The beacon housing stands open, waiting for a light.\n";
const NOTE_TEXT: &str = "The note is in the old keeper's hand: \"Verbs are short here. LOOK to see a room again. EXAMINE what you find. TAKE what you need. INVENTORY lists what you carry. NORTH and SOUTH walk the paths. SAVE keeps your place and RESTORE returns to it. Take the lamp from the storeroom, light it with a match, and carry it up to the stair head.\"\n";
const LAMP_DARK: &str = "A brass storm lamp, cold and unlit.\n";
const LAMP_LIT: &str = "The lamp burns with a small steady flame.\n";
const MATCH_TEXT: &str = "A tin matchbox. A few matches rattle inside.\n";
const ENDING: &str = "You set the burning lamp into the housing. The first beam of the season turns slowly out over the water.\n\nThe watch is yours. The light is lit.\n\n*** You have learned the moves ***\n";

/// Assembles the whole story file.
// Assembly listings are long by nature; splitting the game across helpers
// would scatter the control flow that belongs in one read.
#[allow(clippy::too_many_lines)]
pub fn build_first_light() -> Vec<u8> {
    let mut asm = Asm::new();
    let room = Op::Var(G_ROOM);
    let lamp = Op::Var(G_LAMP);
    let matches = Op::Var(G_MATCH);
    let lit = Op::Var(G_LIT);
    let w1 = Op::Var(G_TMP);
    let w2 = Op::Var(G_TMP2);

    // Entry point: banner, the first room, and then the read loop.
    asm.print(INTRO);
    asm.jump("describe");

    asm.mark("loop");
    asm.new_line();
    asm.print("> ");
    asm.sread();
    asm.inc(G_TURNS);
    asm.loadb(PARSE_BUF, 1, G_TMP2); // word count, kept for noun checks
    asm.jz_var(G_TMP2, "pardon");
    asm.loadw(PARSE_BUF, 1, G_TMP); // first word's dictionary address
    asm.jz_var(G_TMP, "unknown-word");
    asm.je(&[w1, word("save")], "do-save");
    asm.je(&[w1, word("restore")], "do-restore");
    asm.je(&[w1, word("quit")], "do-quit");
    asm.je(&[w1, word("look")], "describe");
    asm.je(&[w1, word("north"), word("n")], "go-north");
    asm.je(&[w1, word("south"), word("s")], "go-south");
    asm.je(&[w1, word("inventory"), word("i")], "inventory");
    asm.je(&[w1, word("examine"), word("read")], "examine");
    asm.je(&[w1, word("take"), word("get")], "take");
    asm.je(&[w1, word("drop")], "drop");
    asm.je(&[w1, word("light")], "light");
    asm.print("That is not something you need to do here.\n");
    asm.jump("loop");

    asm.mark("pardon");
    asm.print("Beg your pardon?\n");
    asm.jump("loop");
    asm.mark("unknown-word");
    asm.print("The story does not know that word.\n");
    asm.jump("loop");

    // Where am I.
    asm.mark("describe");
    asm.je(&[room, Op::Small(0)], "room-gate");
    asm.je(&[room, Op::Small(1)], "room-store");
    asm.jump("room-stair");

    asm.mark("room-gate");
    asm.print(ROOM_GATE);
    asm.jump("loop");

    asm.mark("room-store");
    asm.print(ROOM_STORE);
    asm.je(&[lamp, Op::Small(0)], "show-lamp");
    asm.mark("store-match-check");
    asm.je(&[matches, Op::Small(0)], "show-match");
    asm.mark("store-done");
    asm.jump("loop");
    asm.mark("show-lamp");
    asm.print(LAMP_HERE);
    asm.jump("store-match-check");
    asm.mark("show-match");
    asm.print(MATCH_HERE);
    asm.jump("store-done");

    asm.mark("room-stair");
    asm.je(&[lit, Op::Small(1)], "won");
    asm.print(ROOM_STAIR_DARK);
    asm.jump("loop");
    asm.mark("won");
    asm.print(ENDING);
    asm.quit();

    // Walking.
    asm.mark("go-north");
    asm.je(&[room, Op::Small(0)], "north-to-store");
    asm.je(&[room, Op::Small(1)], "north-to-stair");
    asm.print("No path that way.\n");
    asm.jump("loop");
    asm.mark("north-to-store");
    asm.store(G_ROOM, 1);
    asm.jump("describe");
    asm.mark("north-to-stair");
    asm.store(G_ROOM, 2);
    asm.jump("describe");

    asm.mark("go-south");
    asm.je(&[room, Op::Small(1)], "south-to-gate");
    asm.je(&[room, Op::Small(2)], "south-to-store");
    asm.print("No path that way.\n");
    asm.jump("loop");
    asm.mark("south-to-gate");
    asm.store(G_ROOM, 0);
    asm.jump("describe");
    asm.mark("south-to-store");
    asm.store(G_ROOM, 1);
    asm.jump("describe");

    // What am I carrying.
    asm.mark("inventory");
    asm.je(&[lamp, Op::Small(1)], "list-lamp");
    asm.je(&[matches, Op::Small(1)], "list-match");
    asm.print("You are empty-handed.\n");
    asm.jump("loop");
    asm.mark("list-lamp");
    asm.print("You are carrying:\na brass lamp");
    asm.je(&[lit, Op::Small(1)], "list-lit");
    asm.mark("list-lamp-done");
    asm.new_line();
    asm.je(&[matches, Op::Small(1)], "list-match");
    asm.jump("loop");
    asm.mark("list-lit");
    asm.print(" (lit)");
    asm.jump("list-lamp-done");
    asm.mark("list-match");
    asm.je(&[lamp, Op::Small(1)], "list-match-line");
    asm.print("You are carrying:\n");
    asm.mark("list-match-line");
    asm.print("a matchbox\n");
    asm.jump("loop");

    // Looking closer.
    asm.mark("examine");
    asm.jg_var_small(G_TMP2, 1, "examine-noun");
    asm.print("Examine what?\n");
    asm.jump("loop");
    asm.mark("examine-noun");
    asm.loadw(PARSE_BUF, 3, G_TMP2); // word 2s address is at +6
    asm.jz_var(G_TMP2, "unknown-word");
    asm.je(&[w2, word("note")], "examine-note");
    asm.je(&[w2, word("lamp")], "examine-lamp");
    asm.je(
        &[w2, word("match"), word("matches"), word("matchbox")],
        "examine-match",
    );
    asm.print("You see nothing special about that.\n");
    asm.jump("loop");
    asm.mark("examine-note");
    asm.print(NOTE_TEXT);
    asm.jump("loop");
    asm.mark("examine-lamp");
    asm.je(&[lit, Op::Small(1)], "examine-lit");
    asm.print(LAMP_DARK);
    asm.jump("loop");
    asm.mark("examine-lit");
    asm.print(LAMP_LIT);
    asm.jump("loop");
    asm.mark("examine-match");
    asm.print(MATCH_TEXT);
    asm.jump("loop");

    // Taking.
    asm.mark("take");
    asm.jg_var_small(G_TMP2, 1, "take-noun");
    asm.print("Take what?\n");
    asm.jump("loop");
    asm.mark("take-noun");
    asm.loadw(PARSE_BUF, 3, G_TMP2); // word 2s address is at +6
    asm.jz_var(G_TMP2, "unknown-word");
    asm.je(&[w2, word("lamp")], "take-lamp");
    asm.je(
        &[w2, word("match"), word("matches"), word("matchbox")],
        "take-match",
    );
    asm.print("You cannot take that.\n");
    asm.jump("loop");
    asm.mark("take-lamp");
    asm.je(&[lamp, Op::Small(1)], "have-lamp");
    asm.je(&[room, Op::Small(1)], "take-lamp-here");
    asm.print("There is no lamp here.\n");
    asm.jump("loop");
    asm.mark("take-lamp-here");
    asm.store(G_LAMP, 1);
    asm.print("Taken.\n");
    asm.jump("loop");
    asm.mark("have-lamp");
    asm.print("You already have it.\n");
    asm.jump("loop");
    asm.mark("take-match");
    asm.je(&[matches, Op::Small(1)], "have-match");
    asm.je(&[room, Op::Small(1)], "take-match-here");
    asm.print("There is no matchbox here.\n");
    asm.jump("loop");
    asm.mark("take-match-here");
    asm.store(G_MATCH, 1);
    asm.print("Taken.\n");
    asm.jump("loop");
    asm.mark("have-match");
    asm.print("You already have it.\n");
    asm.jump("loop");

    asm.mark("drop");
    asm.print("Dropping things would only lose them in the rain.\n");
    asm.jump("loop");

    // Lighting the lamp.
    asm.mark("light");
    asm.jg_var_small(G_TMP2, 1, "light-noun");
    asm.print("Light what?\n");
    asm.jump("loop");
    asm.mark("light-noun");
    asm.loadw(PARSE_BUF, 3, G_TMP2); // word 2s address is at +6
    asm.je(&[w2, word("lamp")], "light-lamp");
    asm.print("That will not catch a flame.\n");
    asm.jump("loop");
    asm.mark("light-lamp");
    asm.je(&[lamp, Op::Small(0)], "light-no-lamp");
    asm.je(&[matches, Op::Small(0)], "light-no-match");
    asm.je(&[lit, Op::Small(1)], "light-already");
    asm.store(G_LIT, 1);
    asm.print("The match flares, the wick catches, and the lamp settles into a steady flame.\n");
    asm.jump("loop");
    asm.mark("light-no-lamp");
    asm.print("You are not carrying the lamp.\n");
    asm.jump("loop");
    asm.mark("light-no-match");
    asm.print("You have nothing to strike a match with.\n");
    asm.jump("loop");
    asm.mark("light-already");
    asm.print("The lamp already burns.\n");
    asm.jump("loop");

    // The story's own save and restore, completed by the app.
    asm.mark("do-save");
    asm.save("save-done");
    asm.print("Failed.\n");
    asm.jump("loop");
    asm.mark("save-done");
    asm.print("Ok.\n");
    asm.jump("loop");
    asm.mark("do-restore");
    asm.restore("restore-done");
    asm.print("Failed.\n");
    asm.jump("loop");
    asm.mark("restore-done");
    asm.print("Ok.\n");
    asm.jump("describe");

    asm.mark("do-quit");
    asm.print("The rain keeps time. Goodbye.\n");
    asm.quit();

    let code = asm.link();

    // The whole file. Everything is dynamic memory so a Quetzal save holds
    // the complete state, code included (unchanged) and globals alike.
    let mut file = vec![0u8; usize::from(DICT) + 4];
    file[0] = 3; // version
    file[2..4].copy_from_slice(&1u16.to_be_bytes()); // release
    file[8..10].copy_from_slice(&DICT.to_be_bytes());
    file[0x0c..0x0e].copy_from_slice(&GLOBALS.to_be_bytes());
    file[0x12..0x18].copy_from_slice(b"260919");
    file[usize::from(TEXT_BUF)] = 48; // line capacity
    file[usize::from(PARSE_BUF)] = 8; // word capacity
                                      // Dictionary: no extra separators, four-byte entries.
    file[usize::from(DICT)] = 0;
    file[usize::from(DICT) + 1] = 4;
    file[usize::from(DICT) + 2..usize::from(DICT) + 4]
        .copy_from_slice(&u16::try_from(WORDS.len()).expect("few words").to_be_bytes());
    for known in WORDS {
        file.extend_from_slice(&encode_word(known));
    }
    // Object table: the sixty-two property defaults and not one object. The
    // game keeps its state in globals, so no entry ever points here.
    let objects = file.len();
    file[0x0a..0x0c].copy_from_slice(&u16::try_from(objects).expect("low memory").to_be_bytes());
    file.extend_from_slice(&[0u8; 62]);
    let start = file.len();
    file[6..8].copy_from_slice(&u16::try_from(start).expect("low memory").to_be_bytes());
    file.extend_from_slice(&code);
    let high = file.len();
    let high = u16::try_from(high).expect("low memory");
    file[4..6].copy_from_slice(&high.to_be_bytes());
    file[0x0e..0x10].copy_from_slice(&high.to_be_bytes());
    file[0x1a..0x1c].copy_from_slice(&(high / 2).to_be_bytes());
    let checksum = file[0x40..]
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    file[0x1c..0x1e].copy_from_slice(&checksum.to_be_bytes());
    file
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zvm::{Machine, RunState};

    fn play(script: &[&str]) -> (Machine, String) {
        let mut machine = Machine::new(build_first_light(), "first-light.z3").expect("story opens");
        let mut out = String::new();
        for command in script {
            match machine.run().expect("turn runs") {
                RunState::NeedInput { .. } => {}
                other => panic!("expected input, got {other:?}: {out}"),
            }
            out.push_str(&machine.take_output());
            machine.input(command).expect("command accepted");
        }
        match machine.run().expect("last turn runs") {
            RunState::NeedInput { .. } | RunState::Halted => {}
            other => panic!("unexpected state {other:?}"),
        }
        out.push_str(&machine.take_output());
        (machine, out)
    }

    #[test]
    fn the_tutorial_teaches_and_ends() {
        let (_, out) = play(&[
            "look",
            "examine note",
            "north",
            "take lamp",
            "take matchbox",
            "inventory",
            "light lamp",
            "north",
        ]);
        for expected in [
            "First Light",
            "Gatehouse",
            "old keeper's hand",
            "Storeroom",
            "Taken.",
            "a brass lamp",
            "a matchbox",
            "steady flame",
            "first beam of the season",
            "learned the moves",
        ] {
            assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
        }
    }

    #[test]
    fn wrong_turns_are_kind() {
        let (_, out) = play(&[
            "",
            "frobozz",
            "note",
            "take lamp",
            "north",
            "take lamp",
            "light lamp",
            "south",
            "light lamp",
        ]);
        for expected in [
            "Beg your pardon?",
            "does not know that word",
            "not something you need to do",
            "no lamp here",
            "Taken.",
            "nothing to strike a match",
            "Gatehouse",
        ] {
            assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
        }
    }

    #[test]
    fn a_save_round_trips_mid_story() {
        let bytes = build_first_light();
        let mut machine = Machine::new(bytes.clone(), "first-light.z3").expect("story opens");
        assert!(matches!(machine.run().unwrap(), RunState::NeedInput { .. }));
        machine.input("north").unwrap();
        assert!(matches!(machine.run().unwrap(), RunState::NeedInput { .. }));
        machine.input("take lamp").unwrap();
        assert!(matches!(machine.run().unwrap(), RunState::NeedInput { .. }));
        let save = machine.save_quetzal();
        let mut restored = Machine::new(bytes, "first-light.z3").expect("story reopens");
        restored.restore_quetzal(&save).unwrap();
        assert!(matches!(
            restored.run().unwrap(),
            RunState::NeedInput { .. }
        ));
        restored.input("inventory").unwrap();
        assert!(matches!(
            restored.run().unwrap(),
            RunState::NeedInput { .. }
        ));
        assert!(restored.take_output().contains("a brass lamp"));
    }

    #[test]
    fn the_file_is_well_formed() {
        let bytes = build_first_light();
        assert_eq!(bytes[0], 3);
        let declared = u16::from_be_bytes([bytes[0x1a], bytes[0x1b]]);
        assert_eq!(usize::from(declared) * 2, bytes.len());
        let checksum = bytes[0x40..]
            .iter()
            .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
        assert_eq!(u16::from_be_bytes([bytes[0x1c], bytes[0x1d]]), checksum);
    }
}
