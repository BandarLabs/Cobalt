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
    pub fn complete(&mut self, day: u32) -> bool {
        if !self.due(day) || self.done.contains(&day) {
            return false;
        }
        self.skipped.retain(|saved| *saved != day);
        self.done.push(day);
        true
    }
    pub fn skip(&mut self, day: u32) -> bool {
        if !self.due(day) || self.done.contains(&day) || self.skipped.contains(&day) {
            return false;
        }
        self.skipped.push(day);
        true
    }
    pub fn current_streak(&self, today: u32) -> u32 {
        let mut day = today;
        while self.due(day) && !self.done.contains(&day) && !self.skipped.contains(&day) {
            if day == 0 {
                return 0;
            }
            day -= 1;
        }
        let mut count = 0;
        loop {
            if self.due(day) {
                if self.done.contains(&day) {
                    count += 1;
                } else if !self.skipped.contains(&day) {
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
            if self.done.contains(&day) {
                run += 1;
                best = best.max(run);
            } else if !self.skipped.contains(&day) {
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
    text.split(',')
        .filter_map(|value| value.parse().ok())
        .collect()
}
pub fn decode(bytes: &[u8]) -> Vec<Habit> {
    std::str::from_utf8(bytes)
        .unwrap_or("")
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 5 || fields[2].is_empty() {
                return None;
            }
            let schedule = match fields[1] {
                "d" => Schedule::Daily,
                "w" => Schedule::Weekdays,
                value => Schedule::Every(value.strip_prefix('e')?.parse().ok()?),
            };
            Some(Habit {
                archived: fields[0] == "1",
                schedule,
                name: fields[2].to_owned(),
                done: decode_days(fields[3]),
                skipped: decode_days(fields[4]),
            })
        })
        .collect()
}
