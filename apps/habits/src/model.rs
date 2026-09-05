pub const MAX_HABIT_NAME_CHARS: usize = 48;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Schedule {
    Daily,
    Weekdays,
    Every(u8),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Habit {
    pub name: String,
    pub schedule: Schedule,
    pub archived: bool,
    pub done: Vec<u32>,
    pub skipped: Vec<u32>,
}

impl Habit {
    pub fn new(name: String) -> Self {
        Self {
            name,
            schedule: Schedule::Daily,
            archived: false,
            done: Vec::new(),
            skipped: Vec::new(),
        }
    }
    pub fn due(&self, day: u32) -> bool {
        match self.schedule {
            Schedule::Daily => true,
            // Unix day zero was a Thursday; shift to Monday = 0.
            Schedule::Weekdays => (day + 3) % 7 < 5,
            Schedule::Every(n) => day % u32::from(n.max(1)) == 0,
        }
    }
    pub fn toggle_complete(&mut self, day: u32) -> bool {
        if !self.due(day) {
            return false;
        }
        if let Ok(index) = self.done.binary_search(&day) {
            self.done.remove(index);
            remove_day(&mut self.skipped, day);
            return true;
        }
        remove_day(&mut self.skipped, day);
        insert_day(&mut self.done, day);
        true
    }
    pub fn skip(&mut self, day: u32) -> bool {
        if !self.due(day) || self.skipped.binary_search(&day).is_ok() {
            return false;
        }
        remove_day(&mut self.done, day);
        insert_day(&mut self.skipped, day);
        true
    }
    pub fn current_streak(&self, today: u32) -> u32 {
        let mut day = today;
        let mut count = 0;
        loop {
            if self.due(day) {
                if has_day(&self.done, day) {
                    count += 1;
                } else if !has_day(&self.skipped, day) && day != today {
                    break;
                }
            }
            if day == 0 {
                break;
            }
            day -= 1;
        }
        count
    }
    pub fn best_streak(&self, through: u32) -> u32 {
        let mut best = 0;
        let mut run = 0;
        for day in 0..=through {
            if !self.due(day) {
                continue;
            }
            if has_day(&self.done, day) {
                run += 1;
                best = best.max(run);
            } else if !has_day(&self.skipped, day) {
                run = 0;
            }
        }
        best
    }
    pub fn schedule_label(&self) -> String {
        match self.schedule {
            Schedule::Daily => "daily".into(),
            Schedule::Weekdays => "weekdays".into(),
            Schedule::Every(days) => format!("every {days} days"),
        }
    }
}

pub fn canonical_name(name: &str) -> Option<String> {
    (!name.trim().is_empty() && name == name.trim() && name.chars().count() <= MAX_HABIT_NAME_CHARS)
        .then(|| name.to_owned())
}

fn has_day(days: &[u32], day: u32) -> bool {
    days.binary_search(&day).is_ok()
}

fn insert_day(days: &mut Vec<u32>, day: u32) {
    if let Err(index) = days.binary_search(&day) {
        days.insert(index, day);
    }
}

fn remove_day(days: &mut Vec<u32>, day: u32) {
    if let Ok(index) = days.binary_search(&day) {
        days.remove(index);
    }
}

pub fn encode(habits: &[Habit]) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut encoded = String::new();
    for habit in habits {
        let schedule = match habit.schedule {
            Schedule::Daily => "d".to_owned(),
            Schedule::Weekdays => "w".to_owned(),
            Schedule::Every(n) => format!("e{n}"),
        };
        writeln!(
            encoded,
            "{}\t{}\t{}\t{}\t{}",
            u8::from(habit.archived),
            schedule,
            habit.name.replace(['\t', '\n'], " "),
            days(&habit.done),
            days(&habit.skipped)
        )
        .expect("writing to a string cannot fail");
    }
    encoded.into_bytes()
}
fn days(days: &[u32]) -> String {
    days.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
fn decode_days(text: &str) -> Vec<u32> {
    let mut days: Vec<_> = text
        .split(',')
        .filter_map(|value| value.parse().ok())
        .collect();
    days.sort_unstable();
    days.dedup();
    days
}
pub fn decode_with_blank_names(bytes: &[u8]) -> (Vec<Habit>, usize) {
    let mut blank_names = 0;
    let habits = std::str::from_utf8(bytes)
        .unwrap_or("")
        .lines()
        .filter_map(|line| {
            // A fixed-size array rather than a length check and indexing, so a
            // short line is rejected by the conversion instead of by a rule a
            // later reader has to notice before adding a field.
            let fields: [&str; 5] = line.split('\t').collect::<Vec<_>>().try_into().ok()?;
            if fields[2].trim().is_empty() {
                blank_names += 1;
                return None;
            }
            let archived = match fields[0] {
                "0" => false,
                "1" => true,
                _ => return None,
            };
            let schedule = match fields[1] {
                "d" => Schedule::Daily,
                "w" => Schedule::Weekdays,
                value => Schedule::Every(
                    value
                        .strip_prefix('e')?
                        .parse::<u8>()
                        .ok()
                        .filter(|days| *days > 0)?,
                ),
            };
            let done = decode_days(fields[3]);
            let mut skipped = decode_days(fields[4]);
            skipped.retain(|day| done.binary_search(day).is_err());
            Some(Habit {
                name: fields[2].to_owned(),
                schedule,
                archived,
                done,
                skipped,
            })
        })
        .collect();
    (habits, blank_names)
}

#[cfg(test)]
pub fn decode(bytes: &[u8]) -> Vec<Habit> {
    decode_with_blank_names(bytes).0
}
