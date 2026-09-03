//! A bounded, deterministic Z-machine interpreter for text-only v3, v5 and v8 stories.
//!
//! The implementation follows Z-Machine Standards 1.1. It deliberately omits
//! the v6 graphics model, sound and real-time input, none of which Parser
//! advertises. Timed input is accepted as ordinary turn-based input.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::comparison_chain,
    clippy::double_must_use,
    clippy::match_same_arms,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::verbose_bit_mask
)]

use std::fmt;

const HEADER_LEN: usize = 64;
const MAX_STEPS_PER_TURN: usize = 200_000;
const MAX_STACK_WORDS: usize = 32_768;
const MAX_FRAMES: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoryError {
    TooShort,
    Glulx,
    UnsupportedVersion(u8),
    Invalid(&'static str),
    Fault(String),
}

impl fmt::Display for StoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => formatter.write_str("the file is too short to be a Z-machine story"),
            Self::Glulx => formatter.write_str("this is a Glulx story — not supported yet"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "Z-machine version {version} is not supported")
            }
            Self::Invalid(reason) => write!(formatter, "invalid story file: {reason}"),
            Self::Fault(reason) => write!(formatter, "story stopped: {reason}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoryInfo {
    pub version: u8,
    pub release: u16,
    pub serial: [u8; 6],
    pub checksum: u16,
    pub title: String,
    pub id: String,
}

impl StoryInfo {
    #[must_use]
    pub fn inspect(bytes: &[u8], file_name: &str) -> Result<Self, StoryError> {
        if bytes.starts_with(b"Glul") {
            return Err(StoryError::Glulx);
        }
        if bytes.len() < HEADER_LEN {
            return Err(StoryError::TooShort);
        }
        let version = bytes[0];
        if !matches!(version, 3 | 5 | 8) {
            return Err(StoryError::UnsupportedVersion(version));
        }
        let release = word(bytes, 2)?;
        let serial: [u8; 6] = bytes[0x12..0x18]
            .try_into()
            .map_err(|_| StoryError::Invalid("missing serial number"))?;
        let checksum = word(bytes, 0x1c)?;
        let scale = if version <= 3 {
            2
        } else if version <= 5 {
            4
        } else {
            8
        };
        let declared = usize::from(word(bytes, 0x1a)?).saturating_mul(scale);
        if declared != 0 && declared > bytes.len() {
            return Err(StoryError::Invalid("declared length exceeds the file"));
        }
        let title = file_name
            .rsplit('/')
            .next()
            .unwrap_or(file_name)
            .trim_end_matches(|character: char| {
                character == '.'
                    || character.is_ascii_digit()
                    || matches!(character.to_ascii_lowercase(), 'z')
            })
            .replace(['_', '-'], " ")
            .trim()
            .to_owned();
        let title = if title.is_empty() {
            format!("Story {release}")
        } else {
            title
        };
        let serial_text = String::from_utf8_lossy(&serial);
        let id = format!("{release}-{serial_text}-{checksum:04x}");
        Ok(Self {
            version,
            release,
            serial,
            checksum,
            title,
            id,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunState {
    NeedInput { max_bytes: usize },
    Halted,
}

#[derive(Clone, Debug)]
struct Frame {
    return_pc: usize,
    store: Option<u8>,
    locals: Vec<u16>,
    stack_base: usize,
    argument_count: u8,
}

#[derive(Clone, Debug)]
struct Snapshot {
    memory: Vec<u8>,
    pc: usize,
    stack: Vec<u16>,
    frames: Vec<Frame>,
    rng: u32,
}

#[derive(Clone, Debug)]
pub struct Machine {
    original: Vec<u8>,
    memory: Vec<u8>,
    info: StoryInfo,
    pc: usize,
    stack: Vec<u16>,
    frames: Vec<Frame>,
    output: String,
    status: String,
    halted: bool,
    input: Option<InputRequest>,
    undo: Option<Snapshot>,
    rng: u32,
}

#[derive(Clone, Copy, Debug)]
struct InputRequest {
    text: usize,
    parse: usize,
    store: Option<u8>,
}

#[derive(Clone, Copy, Debug)]
enum Operand {
    Large(u16),
    Small(u8),
    Variable(u8),
}

impl Machine {
    pub fn new(bytes: Vec<u8>, file_name: &str) -> Result<Self, StoryError> {
        let info = StoryInfo::inspect(&bytes, file_name)?;
        let pc = usize::from(word(&bytes, 6)?);
        if pc >= bytes.len() {
            return Err(StoryError::Invalid(
                "initial program counter is outside the file",
            ));
        }
        let mut machine = Self {
            original: bytes.clone(),
            memory: bytes,
            info,
            pc,
            stack: Vec::new(),
            frames: vec![Frame {
                return_pc: 0,
                store: None,
                locals: Vec::new(),
                stack_base: 0,
                argument_count: 0,
            }],
            output: String::new(),
            status: String::new(),
            halted: false,
            input: None,
            undo: None,
            rng: 0x5eed_1234,
        };
        machine.configure_header()?;
        Ok(machine)
    }

    #[must_use]
    pub fn info(&self) -> &StoryInfo {
        &self.info
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.output)
    }

    pub fn run(&mut self) -> Result<RunState, StoryError> {
        if self.halted {
            return Ok(RunState::Halted);
        }
        if let Some(request) = self.input {
            return Ok(RunState::NeedInput {
                max_bytes: self.input_capacity(request.text)?,
            });
        }
        for _ in 0..MAX_STEPS_PER_TURN {
            self.step()?;
            if self.halted {
                return Ok(RunState::Halted);
            }
            if let Some(request) = self.input {
                return Ok(RunState::NeedInput {
                    max_bytes: self.input_capacity(request.text)?,
                });
            }
        }
        Err(StoryError::Fault(
            "instruction budget exhausted before the next input".to_owned(),
        ))
    }

    pub fn input(&mut self, text: &str) -> Result<RunState, StoryError> {
        let request = self
            .input
            .take()
            .ok_or_else(|| StoryError::Fault("the story did not ask for input".to_owned()))?;
        self.write_input(request, text)?;
        if let Some(store) = request.store {
            self.set_variable(store, 13)?;
        }
        self.run()
    }

    #[must_use]
    pub fn save_quetzal(&self) -> Vec<u8> {
        let mut body = Vec::new();
        let mut ifhd = Vec::new();
        ifhd.extend_from_slice(&self.info.release.to_be_bytes());
        ifhd.extend_from_slice(&self.info.serial);
        ifhd.extend_from_slice(&self.info.checksum.to_be_bytes());
        ifhd.push(((self.pc >> 16) & 0xff) as u8);
        ifhd.push(((self.pc >> 8) & 0xff) as u8);
        ifhd.push((self.pc & 0xff) as u8);
        chunk(&mut body, b"IFhd", &ifhd);

        let dynamic = usize::from(self.header_word(0x0e).unwrap_or(0)).min(self.memory.len());
        chunk(&mut body, b"UMem", &self.memory[..dynamic]);

        let mut parser = Vec::new();
        parser.extend_from_slice(b"PARS");
        parser.extend_from_slice(&(self.stack.len() as u32).to_be_bytes());
        for value in &self.stack {
            parser.extend_from_slice(&value.to_be_bytes());
        }
        parser.extend_from_slice(&(self.frames.len() as u16).to_be_bytes());
        for frame in &self.frames {
            parser.extend_from_slice(&(frame.return_pc as u32).to_be_bytes());
            parser.push(frame.store.unwrap_or(u8::MAX));
            parser.push(frame.argument_count);
            parser.extend_from_slice(&(frame.stack_base as u32).to_be_bytes());
            parser.push(frame.locals.len() as u8);
            for local in &frame.locals {
                parser.extend_from_slice(&local.to_be_bytes());
            }
        }
        parser.extend_from_slice(&self.rng.to_be_bytes());
        if let Some(input) = self.input {
            parser.push(1);
            parser.extend_from_slice(&(input.text as u32).to_be_bytes());
            parser.extend_from_slice(&(input.parse as u32).to_be_bytes());
            parser.push(input.store.unwrap_or(u8::MAX));
        } else {
            parser.push(0);
        }
        chunk(&mut body, b"IntD", &parser);

        let length = u32::try_from(body.len().saturating_add(4)).unwrap_or(u32::MAX);
        let mut file = b"FORM".to_vec();
        file.extend_from_slice(&length.to_be_bytes());
        file.extend_from_slice(b"IFZS");
        file.extend_from_slice(&body);
        file
    }

    pub fn restore_quetzal(&mut self, bytes: &[u8]) -> Result<(), StoryError> {
        if bytes.len() < 12 || &bytes[..4] != b"FORM" || &bytes[8..12] != b"IFZS" {
            return Err(StoryError::Invalid("save is not a Quetzal IFZS file"));
        }
        let mut cursor = 12usize;
        let mut header = None;
        let mut dynamic = None;
        let mut parser = None;
        while cursor.saturating_add(8) <= bytes.len() {
            let id = &bytes[cursor..cursor + 4];
            let length = usize::try_from(u32::from_be_bytes(
                bytes[cursor + 4..cursor + 8]
                    .try_into()
                    .map_err(|_| StoryError::Invalid("truncated save chunk"))?,
            ))
            .unwrap_or(usize::MAX);
            cursor += 8;
            let end = cursor
                .checked_add(length)
                .ok_or(StoryError::Invalid("oversized save chunk"))?;
            if end > bytes.len() {
                return Err(StoryError::Invalid("truncated save chunk"));
            }
            match id {
                b"IFhd" => header = Some(&bytes[cursor..end]),
                b"UMem" => dynamic = Some(&bytes[cursor..end]),
                b"IntD" => parser = Some(&bytes[cursor..end]),
                _ => {}
            }
            cursor = end + (length & 1);
        }
        let header = header.ok_or(StoryError::Invalid("save has no IFhd chunk"))?;
        if header.len() < 13
            || header[..2] != self.info.release.to_be_bytes()
            || header[2..8] != self.info.serial
            || header[8..10] != self.info.checksum.to_be_bytes()
        {
            return Err(StoryError::Invalid("save belongs to another story"));
        }
        let dynamic = dynamic.ok_or(StoryError::Invalid("save has no memory chunk"))?;
        let expected = usize::from(self.header_word(0x0e)?);
        if dynamic.len() != expected || expected > self.memory.len() {
            return Err(StoryError::Invalid(
                "save memory size does not match the story",
            ));
        }
        self.memory[..expected].copy_from_slice(dynamic);
        self.pc = (usize::from(header[10]) << 16)
            | (usize::from(header[11]) << 8)
            | usize::from(header[12]);
        self.restore_parser_chunk(
            parser.ok_or(StoryError::Invalid("save has no Parser state chunk"))?,
        )?;
        self.halted = false;
        Ok(())
    }

    fn restore_parser_chunk(&mut self, bytes: &[u8]) -> Result<(), StoryError> {
        if bytes.len() < 10 || &bytes[..4] != b"PARS" {
            return Err(StoryError::Invalid("save has incompatible stack state"));
        }
        let mut cursor = 4;
        let stack_count = read_u32(bytes, &mut cursor)? as usize;
        if stack_count > MAX_STACK_WORDS {
            return Err(StoryError::Invalid("save stack is too large"));
        }
        let mut stack = Vec::with_capacity(stack_count);
        for _ in 0..stack_count {
            stack.push(read_u16(bytes, &mut cursor)?);
        }
        let frame_count = usize::from(read_u16(bytes, &mut cursor)?);
        if frame_count == 0 || frame_count > MAX_FRAMES {
            return Err(StoryError::Invalid("save call stack is invalid"));
        }
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let return_pc = read_u32(bytes, &mut cursor)? as usize;
            let stored = take(bytes, &mut cursor)?;
            let argument_count = take(bytes, &mut cursor)?;
            let stack_base = read_u32(bytes, &mut cursor)? as usize;
            let local_count = usize::from(take(bytes, &mut cursor)?);
            let mut locals = Vec::with_capacity(local_count);
            for _ in 0..local_count {
                locals.push(read_u16(bytes, &mut cursor)?);
            }
            frames.push(Frame {
                return_pc,
                store: (stored != u8::MAX).then_some(stored),
                locals,
                stack_base,
                argument_count,
            });
        }
        self.rng = read_u32(bytes, &mut cursor)?;
        self.input = if take(bytes, &mut cursor)? == 0 {
            None
        } else {
            let text = read_u32(bytes, &mut cursor)? as usize;
            let parse = read_u32(bytes, &mut cursor)? as usize;
            let store = take(bytes, &mut cursor)?;
            Some(InputRequest {
                text,
                parse,
                store: (store != u8::MAX).then_some(store),
            })
        };
        self.stack = stack;
        self.frames = frames;
        Ok(())
    }

    fn configure_header(&mut self) -> Result<(), StoryError> {
        self.write_byte(0x20, 80)?;
        self.write_byte(0x21, 25)?;
        if self.info.version >= 5 {
            let mut flags = self.header_word(0x10)?;
            flags &= !0x0007;
            self.write_word(0x10, flags)?;
            self.write_byte(0x1e, 1)?;
            self.write_byte(0x1f, 1)?;
            self.write_word(0x22, 80)?;
            self.write_word(0x24, 25)?;
            self.write_byte(0x26, 1)?;
            self.write_byte(0x27, 1)?;
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), StoryError> {
        let opcode = self.fetch_byte()?;
        match opcode {
            0xbe if self.info.version >= 5 => {
                let extended = self.fetch_byte()?;
                let operands = self.variable_operands(false)?;
                self.execute_extended(extended, &operands)
            }
            0x00..=0x7f => {
                let left = if opcode & 0x40 == 0 {
                    Operand::Small(self.fetch_byte()?)
                } else {
                    Operand::Variable(self.fetch_byte()?)
                };
                let right = if opcode & 0x20 == 0 {
                    Operand::Small(self.fetch_byte()?)
                } else {
                    Operand::Variable(self.fetch_byte()?)
                };
                self.execute_2op(opcode & 0x1f, &[left, right])
            }
            0x80..=0xaf => {
                let kind = (opcode >> 4) & 3;
                let operand = self.fetch_operand(kind)?;
                self.execute_1op(opcode & 0x0f, operand)
            }
            0xb0..=0xbf => self.execute_0op(opcode & 0x0f),
            0xc0..=0xdf => {
                let operands = self.variable_operands(false)?;
                self.execute_2op(opcode & 0x1f, &operands)
            }
            0xe0..=0xff => {
                let two_types = matches!(opcode & 0x1f, 12 | 26);
                let operands = self.variable_operands(two_types)?;
                self.execute_var(opcode & 0x1f, &operands)
            }
        }
    }

    fn execute_0op(&mut self, opcode: u8) -> Result<(), StoryError> {
        match opcode {
            0 => self.return_from_routine(1),
            1 => self.return_from_routine(0),
            2 => {
                let text = self.decode_zstring(self.pc)?;
                self.pc = text.1;
                self.output.push_str(&text.0);
                Ok(())
            }
            3 => {
                let text = self.decode_zstring(self.pc)?;
                self.pc = text.1;
                self.output.push_str(&text.0);
                self.output.push('\n');
                self.return_from_routine(1)
            }
            4 => Ok(()),
            5 => {
                if self.info.version <= 3 {
                    self.branch(true)
                } else {
                    let store = self.fetch_byte()?;
                    self.set_variable(store, 1)
                }
            }
            6 => {
                if self.info.version <= 3 {
                    self.branch(false)
                } else {
                    let store = self.fetch_byte()?;
                    self.set_variable(store, 0)
                }
            }
            7 => {
                self.pc = usize::from(self.header_word(6)?);
                self.stack.clear();
                self.frames.truncate(1);
                self.input = None;
                Ok(())
            }
            8 => {
                let value = self.pop()?;
                self.return_from_routine(value)
            }
            9 => {
                if self.info.version >= 5 {
                    let _ = self.pop()?;
                }
                Ok(())
            }
            10 => {
                self.halted = true;
                Ok(())
            }
            11 => {
                self.output.push('\n');
                Ok(())
            }
            12 => self.update_status(),
            13 => self.branch(self.header_word(0x1c)? == computed_checksum(&self.original)),
            15 if self.info.version >= 5 => self.branch(true),
            _ => self.unsupported("0OP", opcode),
        }
    }

    fn execute_1op(&mut self, opcode: u8, operand: Operand) -> Result<(), StoryError> {
        let value = self.value(operand)?;
        match opcode {
            0 => self.branch(value == 0),
            1 => {
                let sibling = self.object_relation(value, Relation::Sibling)?;
                let store = self.fetch_byte()?;
                self.set_variable(store, sibling)?;
                self.branch(sibling != 0)
            }
            2 => {
                let child = self.object_relation(value, Relation::Child)?;
                let store = self.fetch_byte()?;
                self.set_variable(store, child)?;
                self.branch(child != 0)
            }
            3 => {
                let parent = self.object_relation(value, Relation::Parent)?;
                let store = self.fetch_byte()?;
                self.set_variable(store, parent)
            }
            4 => {
                let length = self.property_length(usize::from(value))?;
                let store = self.fetch_byte()?;
                self.set_variable(store, length)
            }
            5 => {
                let variable = match operand {
                    Operand::Variable(variable) => variable,
                    _ => value as u8,
                };
                let current = self.peek_variable(variable)?;
                self.set_variable(variable, current.wrapping_add(1))
            }
            6 => {
                let variable = match operand {
                    Operand::Variable(variable) => variable,
                    _ => value as u8,
                };
                let current = self.peek_variable(variable)?;
                self.set_variable(variable, current.wrapping_sub(1))
            }
            7 => {
                let text = self.decode_zstring(usize::from(value))?;
                self.output.push_str(&text.0);
                Ok(())
            }
            8 => {
                let store = self.fetch_byte()?;
                self.call(value, &[], Some(store))
            }
            9 => {
                self.remove_object(value)?;
                Ok(())
            }
            10 => {
                let name = self.object_name(value)?;
                self.output.push_str(&name);
                Ok(())
            }
            11 => self.return_from_routine(value),
            12 => {
                self.pc = self
                    .pc
                    .wrapping_add_signed(isize::from(value as i16))
                    .saturating_sub(2);
                Ok(())
            }
            13 => {
                let packed = self.unpack_string(value);
                let text = self.decode_zstring(packed)?;
                self.output.push_str(&text.0);
                Ok(())
            }
            14 => {
                let byte = self.read_byte(usize::from(value))?;
                let store = self.fetch_byte()?;
                self.set_variable(store, u16::from(byte))
            }
            15 => {
                let word = self.read_word(usize::from(value))?;
                let store = self.fetch_byte()?;
                self.set_variable(store, word)
            }
            _ => self.unsupported("1OP", opcode),
        }
    }

    fn execute_2op(&mut self, opcode: u8, operands: &[Operand]) -> Result<(), StoryError> {
        let values = self.values(operands)?;
        let a = *values.first().unwrap_or(&0);
        let b = *values.get(1).unwrap_or(&0);
        match opcode {
            1 => self.branch(values.iter().skip(1).any(|value| *value == a)),
            2 => self.branch((a as i16) < (b as i16)),
            3 => self.branch((a as i16) > (b as i16)),
            4 => {
                let variable = operand_variable(operands.first(), a);
                let changed = self.peek_variable(variable)?.wrapping_sub(1);
                self.set_variable(variable, changed)?;
                self.branch((changed as i16) < (b as i16))
            }
            5 => {
                let variable = operand_variable(operands.first(), a);
                let changed = self.peek_variable(variable)?.wrapping_add(1);
                self.set_variable(variable, changed)?;
                self.branch((changed as i16) > (b as i16))
            }
            6 => self.branch(self.object_relation(a, Relation::Parent)? == b),
            7 => self.branch(a & b == b),
            8 => self.store_result(a | b),
            9 => self.store_result(a & b),
            10 => self.branch(self.test_attribute(a, b)?),
            11 => {
                self.set_attribute(a, b, true)?;
                Ok(())
            }
            12 => {
                self.set_attribute(a, b, false)?;
                Ok(())
            }
            13 => {
                let variable = match operands.first() {
                    Some(Operand::Variable(variable)) => *variable,
                    _ => a as u8,
                };
                self.set_variable(variable, b)
            }
            14 => {
                self.insert_object(a, b)?;
                Ok(())
            }
            15 => {
                self.store_result(self.read_word(usize::from(a).wrapping_add(usize::from(b) * 2))?)
            }
            16 => self.store_result(u16::from(
                self.read_byte(usize::from(a).wrapping_add(usize::from(b)))?,
            )),
            17 => self.store_result(self.get_property(a, b)?),
            18 => self.store_result(self.property_address(a, b)?.unwrap_or(0) as u16),
            19 => self.store_result(self.next_property(a, b)?),
            20 => self.store_result(a.wrapping_add(b)),
            21 => self.store_result(a.wrapping_sub(b)),
            22 => self.store_result((a as i16).wrapping_mul(b as i16) as u16),
            23 => {
                if b == 0 {
                    return Err(StoryError::Fault("division by zero".to_owned()));
                }
                self.store_result(((a as i16) / (b as i16)) as u16)
            }
            24 => {
                if b == 0 {
                    return Err(StoryError::Fault("remainder by zero".to_owned()));
                }
                self.store_result(((a as i16) % (b as i16)) as u16)
            }
            25 => {
                let store = self.fetch_byte()?;
                self.call(a, &values[1..], Some(store))
            }
            26 => {
                let routine = self.frames.len().saturating_sub(1);
                self.store_result(u16::try_from(routine).unwrap_or(u16::MAX))
            }
            27 => {
                let requested = usize::from(a);
                if requested >= self.frames.len() {
                    return Err(StoryError::Fault("invalid catch token".to_owned()));
                }
                while self.frames.len().saturating_sub(1) > requested {
                    self.frames.pop();
                }
                self.return_from_routine(b)
            }
            28 => {
                let shift = b as i16;
                let result = if shift >= 0 {
                    a.wrapping_shl(u32::from(shift.unsigned_abs().min(15)))
                } else {
                    a.wrapping_shr(u32::from(shift.unsigned_abs().min(15)))
                };
                self.store_result(result)
            }
            29 => {
                let shift = b as i16;
                let result = if shift >= 0 {
                    (a as i16).wrapping_shl(u32::from(shift.unsigned_abs().min(15)))
                } else {
                    (a as i16).wrapping_shr(u32::from(shift.unsigned_abs().min(15)))
                };
                self.store_result(result as u16)
            }
            _ => self.unsupported("2OP", opcode),
        }
    }

    fn execute_var(&mut self, opcode: u8, operands: &[Operand]) -> Result<(), StoryError> {
        let values = self.values(operands)?;
        match opcode {
            0 => {
                let store = self.fetch_byte()?;
                self.call(*values.first().unwrap_or(&0), &values[1..], Some(store))
            }
            1 => {
                let address = usize::from(*values.first().unwrap_or(&0))
                    .wrapping_add(usize::from(*values.get(1).unwrap_or(&0)) * 2);
                self.write_word(address, *values.get(2).unwrap_or(&0))
            }
            2 => {
                let address = usize::from(*values.first().unwrap_or(&0))
                    .wrapping_add(usize::from(*values.get(1).unwrap_or(&0)));
                self.write_byte(address, *values.get(2).unwrap_or(&0) as u8)
            }
            3 => self.put_property(
                *values.first().unwrap_or(&0),
                *values.get(1).unwrap_or(&0),
                *values.get(2).unwrap_or(&0),
            ),
            4 => {
                let text = usize::from(*values.first().unwrap_or(&0));
                let parse = usize::from(*values.get(1).unwrap_or(&0));
                let store = (self.info.version >= 5)
                    .then(|| self.fetch_byte())
                    .transpose()?;
                self.input = Some(InputRequest { text, parse, store });
                Ok(())
            }
            5 => {
                let character =
                    char::from_u32(u32::from(*values.first().unwrap_or(&0))).unwrap_or('\u{fffd}');
                self.output.push(character);
                Ok(())
            }
            6 => {
                let number = *values.first().unwrap_or(&0) as i16;
                self.output.push_str(&number.to_string());
                Ok(())
            }
            7 => {
                let range = *values.first().unwrap_or(&0) as i16;
                let result = if range == 0 {
                    self.rng = 0x5eed_1234;
                    0
                } else if range < 0 {
                    self.rng = u32::from(range.unsigned_abs()).max(1);
                    0
                } else {
                    self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    u16::try_from(self.rng % u32::from(range as u16) + 1).unwrap_or(1)
                };
                self.store_result(result)
            }
            8 => {
                self.push(*values.first().unwrap_or(&0))?;
                Ok(())
            }
            9 => {
                let value = self.pop()?;
                let variable = operands
                    .first()
                    .map_or(0, |operand| operand_variable(Some(operand), 0));
                self.set_variable(variable, value)
            }
            10 => {
                let lines = *values.first().unwrap_or(&0);
                self.branch(lines == 1)
            }
            11 => Ok(()),
            12 => {
                let store = self.fetch_byte()?;
                self.call(*values.first().unwrap_or(&0), &values[1..], Some(store))
            }
            13 => Ok(()),
            14 => Ok(()),
            15 => {
                let table = usize::from(*values.first().unwrap_or(&0));
                let width = usize::from(*values.get(1).unwrap_or(&0));
                let height = usize::from(*values.get(2).unwrap_or(&1));
                let skip = usize::from(*values.get(3).unwrap_or(&0));
                for row in 0..height {
                    for column in 0..width {
                        let byte = self.read_byte(table + row * (width + skip) + column)?;
                        self.output.push(char::from(byte));
                    }
                    if row + 1 < height {
                        self.output.push('\n');
                    }
                }
                Ok(())
            }
            16 => {
                let table = usize::from(*values.first().unwrap_or(&0));
                let width = usize::from(*values.get(1).unwrap_or(&0));
                let height = usize::from(*values.get(2).unwrap_or(&1));
                let skip = usize::from(*values.get(3).unwrap_or(&0));
                for row in 0..height {
                    for column in 0..width {
                        self.write_byte(table + row * (width + skip) + column, b' ')?;
                    }
                }
                Ok(())
            }
            17 => {
                let mut index = 0;
                let Some(character) = self.output.chars().last() else {
                    return self.store_result(0);
                };
                if character == '\n' {
                    index = 1;
                }
                self.store_result(index)
            }
            18 => {
                let mode = *values.first().unwrap_or(&0);
                if mode == u16::MAX {
                    self.output.clear();
                }
                Ok(())
            }
            19 => Ok(()),
            20 => Ok(()),
            21 => Ok(()),
            22 => {
                let store = self.fetch_byte()?;
                self.input = Some(InputRequest {
                    text: usize::from(*values.first().unwrap_or(&0)),
                    parse: 0,
                    store: Some(store),
                });
                Ok(())
            }
            23 => {
                let value = *values.first().unwrap_or(&0);
                let table = usize::from(*values.get(1).unwrap_or(&0));
                let entries = *values.get(2).unwrap_or(&0);
                let form = *values.get(3).unwrap_or(&0);
                self.scan_table(value, table, entries, form)
            }
            24 => self.store_result(u16::from(
                self.frames.last().map_or(0, |frame| frame.argument_count)
                    >= *values.first().unwrap_or(&0) as u8,
            )),
            25 => {
                let store = self.fetch_byte()?;
                self.call(*values.first().unwrap_or(&0), &values[1..], Some(store))
            }
            26 => {
                let store = self.fetch_byte()?;
                self.call(*values.first().unwrap_or(&0), &values[1..], Some(store))
            }
            27 => {
                let count = usize::from(*values.get(2).unwrap_or(&0));
                let source = usize::from(*values.first().unwrap_or(&0));
                let destination = usize::from(*values.get(1).unwrap_or(&0));
                self.copy_table(source, destination, count)
            }
            28 => self.encode_text(&values),
            29 => {
                if values.len() >= 2 {
                    self.tokenize(usize::from(values[0]), usize::from(values[1]))
                } else {
                    Ok(())
                }
            }
            30 => Ok(()),
            31 => self.branch(true),
            _ => self.unsupported("VAR", opcode),
        }
    }

    fn execute_extended(&mut self, opcode: u8, operands: &[Operand]) -> Result<(), StoryError> {
        let values = self.values(operands)?;
        match opcode {
            0 => self.store_result(1),
            1 => self.store_result(0),
            2 => self.store_result(
                *values.first().unwrap_or(&0) << (values.get(1).copied().unwrap_or(0) & 15),
            ),
            3 => self.store_result(
                ((*values.first().unwrap_or(&0) as i16)
                    >> (values.get(1).copied().unwrap_or(0) & 15)) as u16,
            ),
            4 => self.store_result(0),
            9 => {
                self.undo = Some(self.snapshot());
                self.store_result(1)
            }
            10 => {
                let Some(snapshot) = self.undo.clone() else {
                    return self.store_result(0);
                };
                self.restore_snapshot(snapshot);
                self.store_result(2)
            }
            11 => {
                let text = self.decode_zstring(usize::from(*values.first().unwrap_or(&0)))?;
                self.output.push_str(&text.0);
                Ok(())
            }
            12 => {
                let value = self.read_word(usize::from(*values.first().unwrap_or(&0)))?;
                self.store_result(value)
            }
            13 => self.write_word(
                usize::from(*values.first().unwrap_or(&0)),
                *values.get(1).unwrap_or(&0),
            ),
            _ => self.unsupported("EXT", opcode),
        }
    }

    fn store_result(&mut self, value: u16) -> Result<(), StoryError> {
        let store = self.fetch_byte()?;
        self.set_variable(store, value)
    }

    fn call(
        &mut self,
        packed: u16,
        arguments: &[u16],
        store: Option<u8>,
    ) -> Result<(), StoryError> {
        if packed == 0 {
            if let Some(store) = store {
                self.set_variable(store, 0)?;
            }
            return Ok(());
        }
        if self.frames.len() >= MAX_FRAMES {
            return Err(StoryError::Fault("call stack overflow".to_owned()));
        }
        let address = self.unpack_routine(packed);
        let count = usize::from(self.read_byte(address)?);
        if count > 15 {
            return Err(StoryError::Fault("routine has too many locals".to_owned()));
        }
        let mut cursor = address + 1;
        let mut locals = vec![0; count];
        if self.info.version <= 4 {
            for local in &mut locals {
                *local = self.read_word(cursor)?;
                cursor += 2;
            }
        }
        for (local, argument) in locals.iter_mut().zip(arguments) {
            *local = *argument;
        }
        self.frames.push(Frame {
            return_pc: self.pc,
            store,
            locals,
            stack_base: self.stack.len(),
            argument_count: arguments.len().min(8) as u8,
        });
        self.pc = cursor;
        Ok(())
    }

    fn return_from_routine(&mut self, value: u16) -> Result<(), StoryError> {
        if self.frames.len() <= 1 {
            self.halted = true;
            return Ok(());
        }
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| StoryError::Fault("empty call stack".to_owned()))?;
        self.stack.truncate(frame.stack_base);
        self.pc = frame.return_pc;
        if let Some(store) = frame.store {
            self.set_variable(store, value)?;
        }
        Ok(())
    }

    fn branch(&mut self, condition: bool) -> Result<(), StoryError> {
        let first = self.fetch_byte()?;
        let branch_on_true = first & 0x80 != 0;
        let short = first & 0x40 != 0;
        let offset = if short {
            i16::from(first & 0x3f)
        } else {
            let second = self.fetch_byte()?;
            let raw = (u16::from(first & 0x3f) << 8) | u16::from(second);
            if raw & 0x2000 != 0 {
                (raw | 0xc000) as i16
            } else {
                raw as i16
            }
        };
        if condition != branch_on_true {
            return Ok(());
        }
        match offset {
            0 => self.return_from_routine(0),
            1 => self.return_from_routine(1),
            _ => {
                self.pc = self
                    .pc
                    .wrapping_add_signed(isize::from(offset))
                    .saturating_sub(2);
                Ok(())
            }
        }
    }

    fn value(&mut self, operand: Operand) -> Result<u16, StoryError> {
        match operand {
            Operand::Large(value) => Ok(value),
            Operand::Small(value) => Ok(u16::from(value)),
            Operand::Variable(variable) => self.variable(variable),
        }
    }

    fn values(&mut self, operands: &[Operand]) -> Result<Vec<u16>, StoryError> {
        operands
            .iter()
            .copied()
            .map(|operand| self.value(operand))
            .collect()
    }

    fn variable(&mut self, variable: u8) -> Result<u16, StoryError> {
        match variable {
            0 => self.pop(),
            1..=15 => self
                .frames
                .last()
                .and_then(|frame| frame.locals.get(usize::from(variable - 1)))
                .copied()
                .ok_or_else(|| StoryError::Fault(format!("local variable {variable} is absent"))),
            _ => {
                let address = usize::from(self.header_word(0x0c)?) + usize::from(variable - 16) * 2;
                self.read_word(address)
            }
        }
    }

    fn peek_variable(&self, variable: u8) -> Result<u16, StoryError> {
        match variable {
            0 => self
                .stack
                .last()
                .copied()
                .ok_or_else(|| StoryError::Fault("stack underflow".to_owned())),
            1..=15 => self
                .frames
                .last()
                .and_then(|frame| frame.locals.get(usize::from(variable - 1)))
                .copied()
                .ok_or_else(|| StoryError::Fault(format!("local variable {variable} is absent"))),
            _ => {
                let globals = usize::from(self.header_word(0x0c)?);
                self.read_word(globals + usize::from(variable - 16) * 2)
            }
        }
    }

    fn set_variable(&mut self, variable: u8, value: u16) -> Result<(), StoryError> {
        match variable {
            0 => self.push(value),
            1..=15 => {
                let local = self
                    .frames
                    .last_mut()
                    .and_then(|frame| frame.locals.get_mut(usize::from(variable - 1)))
                    .ok_or_else(|| {
                        StoryError::Fault(format!("local variable {variable} is absent"))
                    })?;
                *local = value;
                Ok(())
            }
            _ => {
                let address = usize::from(self.header_word(0x0c)?) + usize::from(variable - 16) * 2;
                self.write_word(address, value)
            }
        }
    }

    fn push(&mut self, value: u16) -> Result<(), StoryError> {
        if self.stack.len() >= MAX_STACK_WORDS {
            return Err(StoryError::Fault("evaluation stack overflow".to_owned()));
        }
        self.stack.push(value);
        Ok(())
    }

    fn pop(&mut self) -> Result<u16, StoryError> {
        let base = self.frames.last().map_or(0, |frame| frame.stack_base);
        if self.stack.len() <= base {
            return Err(StoryError::Fault("evaluation stack underflow".to_owned()));
        }
        self.stack
            .pop()
            .ok_or_else(|| StoryError::Fault("evaluation stack underflow".to_owned()))
    }

    fn variable_operands(&mut self, two_type_bytes: bool) -> Result<Vec<Operand>, StoryError> {
        let mut types = vec![self.fetch_byte()?];
        if two_type_bytes {
            types.push(self.fetch_byte()?);
        }
        let mut operands = Vec::new();
        for byte in types {
            for shift in [6, 4, 2, 0] {
                let kind = (byte >> shift) & 3;
                if kind == 3 {
                    return Ok(operands);
                }
                operands.push(self.fetch_operand(kind)?);
            }
        }
        Ok(operands)
    }

    fn fetch_operand(&mut self, kind: u8) -> Result<Operand, StoryError> {
        match kind {
            0 => Ok(Operand::Large(self.fetch_word()?)),
            1 => Ok(Operand::Small(self.fetch_byte()?)),
            2 => Ok(Operand::Variable(self.fetch_byte()?)),
            _ => Err(StoryError::Fault("omitted operand was read".to_owned())),
        }
    }

    fn fetch_byte(&mut self) -> Result<u8, StoryError> {
        let value = self.read_byte(self.pc)?;
        self.pc += 1;
        Ok(value)
    }

    fn fetch_word(&mut self) -> Result<u16, StoryError> {
        let value = self.read_word(self.pc)?;
        self.pc += 2;
        Ok(value)
    }

    fn read_byte(&self, address: usize) -> Result<u8, StoryError> {
        self.memory
            .get(address)
            .copied()
            .ok_or_else(|| StoryError::Fault(format!("read outside story memory at {address:#x}")))
    }

    fn read_word(&self, address: usize) -> Result<u16, StoryError> {
        word(&self.memory, address).map_err(|_| {
            StoryError::Fault(format!("word read outside story memory at {address:#x}"))
        })
    }

    fn write_byte(&mut self, address: usize, value: u8) -> Result<(), StoryError> {
        let static_base = usize::from(self.header_word(0x0e)?);
        if address >= static_base {
            return Err(StoryError::Fault(format!(
                "write to static memory at {address:#x}"
            )));
        }
        let byte = self
            .memory
            .get_mut(address)
            .ok_or_else(|| StoryError::Fault(format!("write outside memory at {address:#x}")))?;
        *byte = value;
        Ok(())
    }

    fn write_word(&mut self, address: usize, value: u16) -> Result<(), StoryError> {
        let [high, low] = value.to_be_bytes();
        self.write_byte(address, high)?;
        self.write_byte(address + 1, low)
    }

    fn header_word(&self, address: usize) -> Result<u16, StoryError> {
        self.read_word(address)
    }

    fn unpack_routine(&self, packed: u16) -> usize {
        match self.info.version {
            3 => usize::from(packed) * 2,
            5 => usize::from(packed) * 4,
            8 => usize::from(packed) * 8,
            _ => unreachable!(),
        }
    }

    fn unpack_string(&self, packed: u16) -> usize {
        self.unpack_routine(packed)
    }

    fn decode_zstring(&self, address: usize) -> Result<(String, usize), StoryError> {
        let mut words = Vec::new();
        let mut cursor = address;
        loop {
            let encoded = self.read_word(cursor)?;
            cursor += 2;
            words.push((encoded >> 10) & 0x1f);
            words.push((encoded >> 5) & 0x1f);
            words.push(encoded & 0x1f);
            if encoded & 0x8000 != 0 {
                break;
            }
            if words.len() > 16_384 {
                return Err(StoryError::Fault("unterminated Z-string".to_owned()));
            }
        }
        let mut output = String::new();
        let mut alphabet = 0usize;
        let mut shift_once = false;
        let mut index = 0;
        while index < words.len() {
            let zchar = words[index] as u8;
            index += 1;
            match zchar {
                0 => output.push(' '),
                1..=3 if self.info.version >= 3 => {
                    if index >= words.len() {
                        break;
                    }
                    let next = words[index] as usize;
                    index += 1;
                    let abbreviation = usize::from(zchar - 1) * 32 + next;
                    let table = usize::from(self.header_word(0x18)?);
                    let packed = self.read_word(table + abbreviation * 2)?;
                    let decoded = self.decode_zstring(usize::from(packed) * 2)?;
                    output.push_str(&decoded.0);
                }
                4 => {
                    alphabet = 1;
                    shift_once = true;
                }
                5 => {
                    alphabet = 2;
                    shift_once = true;
                }
                6 if alphabet == 2 => {
                    if index + 1 >= words.len() {
                        break;
                    }
                    let code = (words[index] << 5) | words[index + 1];
                    index += 2;
                    output.push(zscii_char(code));
                    if shift_once {
                        alphabet = 0;
                        shift_once = false;
                    }
                }
                6..=31 => {
                    let character = alphabet_char(alphabet, zchar);
                    output.push(character);
                    if shift_once {
                        alphabet = 0;
                        shift_once = false;
                    }
                }
                _ => {}
            }
        }
        Ok((output, cursor))
    }

    fn input_capacity(&self, address: usize) -> Result<usize, StoryError> {
        let capacity = usize::from(self.read_byte(address)?);
        Ok(if self.info.version <= 4 {
            capacity.saturating_sub(1)
        } else {
            capacity
        })
    }

    fn write_input(&mut self, request: InputRequest, text: &str) -> Result<(), StoryError> {
        let capacity = self.input_capacity(request.text)?;
        let mut bytes = text
            .to_ascii_lowercase()
            .bytes()
            .filter(|byte| byte.is_ascii() && *byte >= b' ')
            .take(capacity)
            .collect::<Vec<_>>();
        if self.info.version <= 4 {
            for (index, byte) in bytes.iter().enumerate() {
                self.write_byte(request.text + 1 + index, *byte)?;
            }
            self.write_byte(request.text + 1 + bytes.len(), 0)?;
        } else {
            self.write_byte(request.text + 1, bytes.len() as u8)?;
            for (index, byte) in bytes.iter().enumerate() {
                self.write_byte(request.text + 2 + index, *byte)?;
            }
        }
        if request.parse != 0 {
            self.tokenize_bytes(request.parse, &mut bytes)?;
        }
        Ok(())
    }

    fn tokenize(&mut self, text: usize, parse: usize) -> Result<(), StoryError> {
        let length = if self.info.version <= 4 {
            let mut length = 0;
            while self.read_byte(text + 1 + length)? != 0 {
                length += 1;
            }
            length
        } else {
            usize::from(self.read_byte(text + 1)?)
        };
        let start = if self.info.version <= 4 {
            text + 1
        } else {
            text + 2
        };
        let mut bytes = (0..length)
            .map(|index| self.read_byte(start + index))
            .collect::<Result<Vec<_>, _>>()?;
        self.tokenize_bytes(parse, &mut bytes)
    }

    fn tokenize_bytes(&mut self, parse: usize, bytes: &mut [u8]) -> Result<(), StoryError> {
        let dictionary = usize::from(self.header_word(8)?);
        let separator_count = usize::from(self.read_byte(dictionary)?);
        let separators = (0..separator_count)
            .map(|index| self.read_byte(dictionary + 1 + index))
            .collect::<Result<Vec<_>, _>>()?;
        let max_words = usize::from(self.read_byte(parse)?);
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < bytes.len() && tokens.len() < max_words {
            while index < bytes.len() && bytes[index] == b' ' {
                index += 1;
            }
            if index >= bytes.len() {
                break;
            }
            let start = index;
            if separators.contains(&bytes[index]) {
                index += 1;
            } else {
                while index < bytes.len()
                    && bytes[index] != b' '
                    && !separators.contains(&bytes[index])
                {
                    index += 1;
                }
            }
            tokens.push((start, index - start));
        }
        self.write_byte(parse + 1, tokens.len() as u8)?;
        for (slot, (start, length)) in tokens.into_iter().enumerate() {
            let encoded = encode_dictionary_word(&bytes[start..start + length], self.info.version);
            let address = self.dictionary_lookup(dictionary, &encoded)?;
            let entry = parse + 2 + slot * 4;
            self.write_word(entry, u16::try_from(address).unwrap_or(0))?;
            self.write_byte(entry + 2, length as u8)?;
            self.write_byte(
                entry + 3,
                u8::try_from(start + if self.info.version <= 4 { 1 } else { 2 }).unwrap_or(u8::MAX),
            )?;
        }
        Ok(())
    }

    fn dictionary_lookup(&self, dictionary: usize, encoded: &[u8]) -> Result<usize, StoryError> {
        let separator_count = usize::from(self.read_byte(dictionary)?);
        let header = dictionary + 1 + separator_count;
        let entry_length = usize::from(self.read_byte(header)?);
        let count = usize::from(self.read_word(header + 1)?);
        let entries = header + 3;
        for index in 0..count {
            let address = entries + index * entry_length;
            if self.memory.get(address..address + encoded.len()) == Some(encoded) {
                return Ok(address);
            }
        }
        Ok(0)
    }

    fn update_status(&mut self) -> Result<(), StoryError> {
        let globals = usize::from(self.header_word(0x0c)?);
        let location = self.read_word(globals)?;
        let left = self.object_name(location)?;
        let first = self.read_word(globals + 2)? as i16;
        let second = self.read_word(globals + 4)? as i16;
        self.status = if self.memory[1] & 0x02 == 0 {
            format!("{left}   Score {first}   Turns {second}")
        } else {
            format!("{left}   {first:02}:{second:02}")
        };
        Ok(())
    }

    fn object_name(&self, object: u16) -> Result<String, StoryError> {
        if object == 0 {
            return Ok(String::new());
        }
        let property = self.object_property_table(object)?;
        let words = usize::from(self.read_byte(property)?);
        if words == 0 {
            return Ok(String::new());
        }
        self.decode_zstring(property + 1).map(|decoded| decoded.0)
    }

    fn object_entry(&self, object: u16) -> Result<usize, StoryError> {
        if object == 0 {
            return Err(StoryError::Fault("object zero has no entry".to_owned()));
        }
        let table = usize::from(self.header_word(0x0a)?);
        let (defaults, size) = if self.info.version <= 3 {
            (62, 9)
        } else {
            (126, 14)
        };
        Ok(table + defaults + (usize::from(object) - 1) * size)
    }

    fn object_relation(&self, object: u16, relation: Relation) -> Result<u16, StoryError> {
        let entry = self.object_entry(object)?;
        if self.info.version <= 3 {
            let offset = match relation {
                Relation::Parent => 4,
                Relation::Sibling => 5,
                Relation::Child => 6,
            };
            Ok(u16::from(self.read_byte(entry + offset)?))
        } else {
            let offset = match relation {
                Relation::Parent => 6,
                Relation::Sibling => 8,
                Relation::Child => 10,
            };
            self.read_word(entry + offset)
        }
    }

    fn set_relation(
        &mut self,
        object: u16,
        relation: Relation,
        value: u16,
    ) -> Result<(), StoryError> {
        let entry = self.object_entry(object)?;
        if self.info.version <= 3 {
            let offset = match relation {
                Relation::Parent => 4,
                Relation::Sibling => 5,
                Relation::Child => 6,
            };
            self.write_byte(entry + offset, value as u8)
        } else {
            let offset = match relation {
                Relation::Parent => 6,
                Relation::Sibling => 8,
                Relation::Child => 10,
            };
            self.write_word(entry + offset, value)
        }
    }

    fn object_property_table(&self, object: u16) -> Result<usize, StoryError> {
        let entry = self.object_entry(object)?;
        let offset = if self.info.version <= 3 { 7 } else { 12 };
        Ok(usize::from(self.read_word(entry + offset)?))
    }

    fn remove_object(&mut self, object: u16) -> Result<(), StoryError> {
        let parent = self.object_relation(object, Relation::Parent)?;
        if parent == 0 {
            return Ok(());
        }
        let first = self.object_relation(parent, Relation::Child)?;
        if first == object {
            let sibling = self.object_relation(object, Relation::Sibling)?;
            self.set_relation(parent, Relation::Child, sibling)?;
        } else {
            let mut previous = first;
            while previous != 0 {
                let sibling = self.object_relation(previous, Relation::Sibling)?;
                if sibling == object {
                    let next = self.object_relation(object, Relation::Sibling)?;
                    self.set_relation(previous, Relation::Sibling, next)?;
                    break;
                }
                previous = sibling;
            }
        }
        self.set_relation(object, Relation::Parent, 0)?;
        self.set_relation(object, Relation::Sibling, 0)
    }

    fn insert_object(&mut self, object: u16, destination: u16) -> Result<(), StoryError> {
        self.remove_object(object)?;
        let child = self.object_relation(destination, Relation::Child)?;
        self.set_relation(object, Relation::Parent, destination)?;
        self.set_relation(object, Relation::Sibling, child)?;
        self.set_relation(destination, Relation::Child, object)
    }

    fn test_attribute(&self, object: u16, attribute: u16) -> Result<bool, StoryError> {
        let entry = self.object_entry(object)?;
        let limit = if self.info.version <= 3 { 32 } else { 48 };
        if attribute >= limit {
            return Err(StoryError::Fault(
                "attribute number is out of range".to_owned(),
            ));
        }
        let byte = self.read_byte(entry + usize::from(attribute / 8))?;
        Ok(byte & (0x80 >> (attribute % 8)) != 0)
    }

    fn set_attribute(
        &mut self,
        object: u16,
        attribute: u16,
        enabled: bool,
    ) -> Result<(), StoryError> {
        let entry = self.object_entry(object)?;
        let offset = entry + usize::from(attribute / 8);
        let mask = 0x80 >> (attribute % 8);
        let current = self.read_byte(offset)?;
        self.write_byte(
            offset,
            if enabled {
                current | mask
            } else {
                current & !mask
            },
        )
    }

    fn property_address(&self, object: u16, wanted: u16) -> Result<Option<usize>, StoryError> {
        let table = self.object_property_table(object)?;
        let name_words = usize::from(self.read_byte(table)?);
        let mut cursor = table + 1 + name_words * 2;
        loop {
            let first = self.read_byte(cursor)?;
            if first == 0 {
                return Ok(None);
            }
            let (number, length, header) = if self.info.version <= 3 {
                (u16::from(first & 0x1f), usize::from(first >> 5) + 1, 1)
            } else if first & 0x80 != 0 {
                let second = self.read_byte(cursor + 1)?;
                (
                    u16::from(first & 0x3f),
                    usize::from(second & 0x3f).max(64 * usize::from(second & 0x3f == 0)),
                    2,
                )
            } else {
                (u16::from(first & 0x3f), usize::from((first >> 6) + 1), 1)
            };
            if number == wanted {
                return Ok(Some(cursor + header));
            }
            cursor += header + length;
        }
    }

    fn property_length(&self, address: usize) -> Result<u16, StoryError> {
        if address == 0 {
            return Ok(0);
        }
        let byte = self.read_byte(address - 1)?;
        if self.info.version <= 3 {
            Ok(u16::from((byte >> 5) + 1))
        } else if byte & 0x80 != 0 {
            Ok(u16::from(byte & 0x3f).max(64 * u16::from(byte & 0x3f == 0)))
        } else {
            Ok(u16::from((byte >> 6) + 1))
        }
    }

    fn get_property(&self, object: u16, property: u16) -> Result<u16, StoryError> {
        if let Some(address) = self.property_address(object, property)? {
            if self.property_length(address)? == 1 {
                Ok(u16::from(self.read_byte(address)?))
            } else {
                self.read_word(address)
            }
        } else {
            let table = usize::from(self.header_word(0x0a)?);
            self.read_word(table + usize::from(property.saturating_sub(1)) * 2)
        }
    }

    fn put_property(&mut self, object: u16, property: u16, value: u16) -> Result<(), StoryError> {
        let address = self
            .property_address(object, property)?
            .ok_or_else(|| StoryError::Fault("put_prop named a missing property".to_owned()))?;
        if self.property_length(address)? == 1 {
            self.write_byte(address, value as u8)
        } else {
            self.write_word(address, value)
        }
    }

    fn next_property(&self, object: u16, property: u16) -> Result<u16, StoryError> {
        let table = self.object_property_table(object)?;
        let name_words = usize::from(self.read_byte(table)?);
        let mut cursor = table + 1 + name_words * 2;
        loop {
            let first = self.read_byte(cursor)?;
            if first == 0 {
                return Ok(0);
            }
            let number = u16::from(first & if self.info.version <= 3 { 0x1f } else { 0x3f });
            let address = self.property_address(object, number)?.ok_or_else(|| {
                StoryError::Fault("property list changed while reading".to_owned())
            })?;
            let length = usize::from(self.property_length(address)?);
            if property == 0 {
                return Ok(number);
            }
            if number == property {
                let next = self.read_byte(address + length)?;
                return Ok(u16::from(
                    next & if self.info.version <= 3 { 0x1f } else { 0x3f },
                ));
            }
            cursor = address + length;
        }
    }

    fn scan_table(
        &mut self,
        value: u16,
        table: usize,
        entries: u16,
        form: u16,
    ) -> Result<(), StoryError> {
        let word_entries = form == 0 || form & 0x80 != 0;
        let length = usize::from(if form == 0 { 0x82 } else { form } & 0x7f);
        let mut found = 0;
        for index in 0..usize::from(entries) {
            let address = table + index * length;
            let candidate = if word_entries {
                self.read_word(address)?
            } else {
                u16::from(self.read_byte(address)?)
            };
            if candidate == value {
                found = u16::try_from(address).unwrap_or(0);
                break;
            }
        }
        self.store_result(found)?;
        self.branch(found != 0)
    }

    fn copy_table(
        &mut self,
        source: usize,
        destination: usize,
        count: usize,
    ) -> Result<(), StoryError> {
        if destination == 0 {
            for index in 0..count {
                self.write_byte(source + index, 0)?;
            }
            return Ok(());
        }
        let bytes = (0..count)
            .map(|index| self.read_byte(source + index))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, byte) in bytes.into_iter().enumerate() {
            self.write_byte(destination + index, byte)?;
        }
        Ok(())
    }

    fn encode_text(&mut self, values: &[u16]) -> Result<(), StoryError> {
        if values.len() < 4 {
            return Ok(());
        }
        let text = usize::from(values[0]) + usize::from(values[2]);
        let length = usize::from(values[1]);
        let destination = usize::from(values[3]);
        let bytes = (0..length)
            .map(|index| self.read_byte(text + index))
            .collect::<Result<Vec<_>, _>>()?;
        let encoded = encode_dictionary_word(&bytes, self.info.version);
        for (index, byte) in encoded.into_iter().enumerate() {
            self.write_byte(destination + index, byte)?;
        }
        Ok(())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            memory: self.memory.clone(),
            pc: self.pc,
            stack: self.stack.clone(),
            frames: self.frames.clone(),
            rng: self.rng,
        }
    }

    fn restore_snapshot(&mut self, snapshot: Snapshot) {
        self.memory = snapshot.memory;
        self.pc = snapshot.pc;
        self.stack = snapshot.stack;
        self.frames = snapshot.frames;
        self.rng = snapshot.rng;
        self.input = None;
        self.halted = false;
    }

    fn unsupported(&self, form: &str, opcode: u8) -> Result<(), StoryError> {
        Err(StoryError::Fault(format!(
            "unsupported {form} opcode {opcode:#04x} at {:#x}",
            self.pc.saturating_sub(1)
        )))
    }
}

#[derive(Clone, Copy)]
enum Relation {
    Parent,
    Sibling,
    Child,
}

fn operand_variable(operand: Option<&Operand>, fallback: u16) -> u8 {
    match operand {
        Some(Operand::Variable(variable)) => *variable,
        _ => fallback as u8,
    }
}

fn word(bytes: &[u8], address: usize) -> Result<u16, StoryError> {
    let pair = bytes
        .get(address..address.saturating_add(2))
        .ok_or(StoryError::Invalid("truncated word"))?;
    Ok(u16::from_be_bytes(
        pair.try_into()
            .map_err(|_| StoryError::Invalid("truncated word"))?,
    ))
}

fn computed_checksum(bytes: &[u8]) -> u16 {
    bytes
        .get(0x40..)
        .unwrap_or_default()
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)))
}

fn zscii_char(code: u16) -> char {
    match code {
        13 => '\n',
        32..=126 => char::from(code as u8),
        155..=223 => '�',
        _ => ' ',
    }
}

fn alphabet_char(alphabet: usize, zchar: u8) -> char {
    const A0: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
    const A1: &[u8; 26] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const A2: &[u8; 26] = b" \n0123456789.,!?_#'\"/\\-:()";
    let index = usize::from(zchar.saturating_sub(6)).min(25);
    char::from(match alphabet {
        1 => A1[index],
        2 => A2[index],
        _ => A0[index],
    })
}

fn encode_dictionary_word(bytes: &[u8], version: u8) -> Vec<u8> {
    let wanted = if version <= 3 { 6 } else { 9 };
    let mut zchars = Vec::new();
    for byte in bytes.iter().copied().take(wanted) {
        if byte.is_ascii_lowercase() {
            zchars.push(byte - b'a' + 6);
        } else if byte.is_ascii_uppercase() {
            zchars.extend([4, byte - b'A' + 6]);
        } else if byte == b' ' {
            zchars.push(0);
        } else {
            zchars.extend([5, 6, byte >> 5, byte & 0x1f]);
        }
    }
    zchars.resize(wanted, 5);
    let mut encoded = Vec::new();
    for (index, group) in zchars.chunks(3).enumerate() {
        let mut value =
            (u16::from(group[0]) << 10) | (u16::from(group[1]) << 5) | u16::from(group[2]);
        if index + 1 == wanted / 3 {
            value |= 0x8000;
        }
        encoded.extend_from_slice(&value.to_be_bytes());
    }
    encoded
}

fn chunk(output: &mut Vec<u8>, id: &[u8; 4], bytes: &[u8]) {
    output.extend_from_slice(id);
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    if bytes.len() & 1 != 0 {
        output.push(0);
    }
}

fn take(bytes: &[u8], cursor: &mut usize) -> Result<u8, StoryError> {
    let value = bytes
        .get(*cursor)
        .copied()
        .ok_or(StoryError::Invalid("truncated save state"))?;
    *cursor += 1;
    Ok(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, StoryError> {
    let high = take(bytes, cursor)?;
    let low = take(bytes, cursor)?;
    Ok(u16::from_be_bytes([high, low]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, StoryError> {
    let a = take(bytes, cursor)?;
    let b = take(bytes, cursor)?;
    let c = take(bytes, cursor)?;
    let d = take(bytes, cursor)?;
    Ok(u32::from_be_bytes([a, b, c, d]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn story(version: u8) -> Vec<u8> {
        let mut bytes = vec![0; 0x300];
        bytes[0] = version;
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        bytes[6..8].copy_from_slice(&0x40u16.to_be_bytes());
        bytes[8..10].copy_from_slice(&0x180u16.to_be_bytes());
        bytes[0x0a..0x0c].copy_from_slice(&0x100u16.to_be_bytes());
        bytes[0x0c..0x0e].copy_from_slice(&0x140u16.to_be_bytes());
        bytes[0x0e..0x10].copy_from_slice(&0x200u16.to_be_bytes());
        bytes[0x12..0x18].copy_from_slice(b"260901");
        let scale = if version == 3 {
            2
        } else if version == 5 {
            4
        } else {
            8
        };
        let declared = u16::try_from(bytes.len() / scale).unwrap();
        bytes[0x1a..0x1c].copy_from_slice(&declared.to_be_bytes());
        let checksum = computed_checksum(&bytes);
        bytes[0x1c..0x1e].copy_from_slice(&checksum.to_be_bytes());

        let mut pc = 0x40;
        bytes[pc] = 0xb2;
        pc += 1;
        let encoded = encode_dictionary_word(b"hello", version);
        bytes[pc..pc + encoded.len()].copy_from_slice(&encoded);
        pc += encoded.len();
        bytes[pc] = 0xbb;
        pc += 1;
        bytes[pc] = 0xba;
        bytes
    }

    fn input_story() -> Vec<u8> {
        let mut bytes = story(5);
        bytes[0x40] = 0xe4;
        bytes[0x41] = 0x5f;
        bytes[0x42] = 0x80;
        bytes[0x43] = 0xa0;
        bytes[0x44] = 0x10;
        bytes[0x45] = 0xb2;
        let encoded = encode_dictionary_word(b"accepted", 5);
        bytes[0x46..0x46 + encoded.len()].copy_from_slice(&encoded);
        bytes[0x46 + encoded.len()] = 0xba;
        bytes[0x80] = 32;
        bytes[0xa0] = 4;
        bytes[0x180] = 0;
        bytes[0x181] = 6;
        bytes[0x182..0x184].copy_from_slice(&0u16.to_be_bytes());
        bytes
    }

    #[test]
    fn deterministic_v3_v5_v8_stories_execute() {
        for version in [3, 5, 8] {
            let mut machine = Machine::new(story(version), "fixture.z3").unwrap();
            assert_eq!(machine.run().unwrap(), RunState::Halted);
            assert_eq!(machine.take_output(), "hello\n");
        }
    }

    #[test]
    fn quetzal_round_trip_restores_dynamic_memory_and_execution() {
        let bytes = story(5);
        let mut machine = Machine::new(bytes.clone(), "fixture.z5").unwrap();
        machine.pc = 0x42;
        machine.write_byte(0x80, 99).unwrap();
        machine.push(7).unwrap();
        let save = machine.save_quetzal();

        let mut restored = Machine::new(bytes, "fixture.z5").unwrap();
        restored.restore_quetzal(&save).unwrap();
        assert_eq!(restored.pc, 0x42);
        assert_eq!(restored.read_byte(0x80).unwrap(), 99);
        assert_eq!(restored.stack, vec![7]);
    }

    #[test]
    fn input_is_bounded_tokenized_and_survives_an_awaiting_input_save() {
        let bytes = input_story();
        let mut machine = Machine::new(bytes.clone(), "input.z5").unwrap();
        assert_eq!(
            machine.run().unwrap(),
            RunState::NeedInput { max_bytes: 32 }
        );
        let save = machine.save_quetzal();
        let mut restored = Machine::new(bytes, "input.z5").unwrap();
        restored.restore_quetzal(&save).unwrap();
        assert_eq!(
            restored.run().unwrap(),
            RunState::NeedInput { max_bytes: 32 }
        );
        assert_eq!(restored.input("LOOK NORTH").unwrap(), RunState::Halted);
        assert_eq!(restored.read_byte(0x81).unwrap(), 10);
        assert_eq!(restored.read_byte(0xa1).unwrap(), 2);
        assert_eq!(restored.take_output(), "accepted");
    }

    #[test]
    fn unsupported_formats_are_named() {
        assert_eq!(
            StoryInfo::inspect(b"Glul\x00\x00\x00\x00", "game.ulx"),
            Err(StoryError::Glulx)
        );
        let mut bytes = vec![0; HEADER_LEN];
        bytes[0] = 6;
        assert_eq!(
            StoryInfo::inspect(&bytes, "game.z6"),
            Err(StoryError::UnsupportedVersion(6))
        );
    }
}
