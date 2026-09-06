#[path = "../src/model.rs"]
mod model;
use kobo_sdk::{action_id, ScreenBuilder};
use kobo_ui::{Chrome, CLARA_BW_METRICS};
use model::*;
#[test]
fn custom_schedule_and_skip_keep_streak_honest() {
    let mut habit = Habit::new("Read".into());
    habit.schedule = Schedule::Every(2);
    for day in [0, 2, 4] {
        assert!(habit.toggle_complete(day));
    }
    assert_eq!(habit.current_streak(4), 3);
    assert!(habit.skip(6));
    assert!(habit.toggle_complete(8));
    assert_eq!(habit.current_streak(8), 4);
    assert_eq!(habit.best_streak(8), 4);
}
#[test]
fn persisted_habits_round_trip() {
    let mut habit = Habit::new("Walk".into());
    habit.toggle_complete(12);
    assert_eq!(decode(&encode(&[habit.clone()])), vec![habit]);
}
#[test]
fn new_habit_names_are_bounded_while_legacy_names_round_trip_exactly() {
    let longest = "x".repeat(MAX_HABIT_NAME_CHARS);
    let legacy_long = "x".repeat(513);
    let legacy_spaced = "  Read before bed  ";

    assert_eq!(canonical_name(&longest), Some(longest.clone()));
    assert!(canonical_name(&legacy_long).is_none());

    let saved = format!("0\td\t{legacy_long}\t5\t\n0\tw\t{legacy_spaced}\t\t6\n0\td\t   \t\t\n");
    let (habits, blank_names) = decode_with_blank_names(saved.as_bytes());
    assert_eq!(blank_names, 1);
    assert_eq!(habits[0].name, legacy_long);
    assert_eq!(habits[1].name, legacy_spaced);
    assert_eq!(decode(&encode(&habits)), habits);
}
#[test]
fn corrupted_saved_habits_are_rejected_or_canonicalized() {
    let saved = b"0\te0\tBroken\t1\t\n2\td\tBroken archive\t1\t\n1\tw\tWalk\t4,2,4\t2,3,3\n";
    assert_eq!(
        decode(saved),
        vec![Habit {
            name: "Walk".into(),
            schedule: Schedule::Weekdays,
            archived: true,
            done: vec![2, 4],
            skipped: vec![3],
        }]
    );
}
#[test]
fn large_corrupt_day_lists_are_deduplicated_and_disjoint() {
    const DAYS: u32 = 20_000;
    let done = (0..DAYS)
        .rev()
        .chain((0..DAYS).rev())
        .map(|day| day.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let skipped = (0..=DAYS + 1)
        .rev()
        .chain((0..=DAYS + 1).rev())
        .map(|day| day.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let saved = format!("0\td\tWalk\t{done}\t{skipped}\n");

    let habits = decode(saved.as_bytes());

    assert_eq!(habits.len(), 1);
    assert_eq!(habits[0].done.len(), DAYS as usize);
    assert_eq!(habits[0].done.first(), Some(&0));
    assert_eq!(habits[0].done.last(), Some(&(DAYS - 1)));
    assert_eq!(habits[0].skipped, vec![DAYS, DAYS + 1]);
    assert_eq!(habits[0].current_streak(DAYS - 1), DAYS);
    assert_eq!(habits[0].best_streak(DAYS - 1), DAYS);
}
#[test]
fn completion_and_skip_transitions_stay_mutually_exclusive() {
    let mut habit = Habit::new("Walk".into());

    assert!(habit.toggle_complete(4));
    assert_eq!(habit.done, vec![4]);
    assert!(habit.toggle_complete(4));
    assert!(habit.done.is_empty());
    assert!(habit.skipped.is_empty());

    assert!(habit.skip(4));
    assert_eq!(habit.skipped, vec![4]);
    assert!(habit.toggle_complete(4));
    assert_eq!(habit.done, vec![4]);
    assert!(habit.skipped.is_empty());

    assert!(habit.skip(4));
    assert!(habit.done.is_empty());
    assert_eq!(habit.skipped, vec![4]);
}
#[test]
fn current_streak_only_excuses_todays_pending_occurrence() {
    let mut daily = Habit::new("Daily".into());
    daily.done = vec![3];
    assert_eq!(daily.current_streak(5), 0, "day 4 was missed");

    let mut weekdays = Habit::new("Weekdays".into());
    weekdays.schedule = Schedule::Weekdays;
    weekdays.done = vec![6];
    assert_eq!(weekdays.current_streak(8), 0, "day 7 was missed");

    let mut alternate_days = Habit::new("Alternate".into());
    alternate_days.schedule = Schedule::Every(2);
    alternate_days.done = vec![2];
    assert_eq!(alternate_days.current_streak(6), 0, "day 4 was missed");
}
#[test]
fn schedules_have_human_readable_labels() {
    let mut habit = Habit::new("Walk".into());
    habit.schedule = Schedule::Weekdays;
    assert_eq!(habit.schedule_label(), "weekdays");
}

#[test]
fn weekday_schedule_uses_the_unix_epoch_weekday() {
    let mut habit = Habit::new("Walk".into());
    habit.schedule = Schedule::Weekdays;
    assert!(habit.due(0), "1970-01-01 was Thursday");
    assert!(habit.due(1), "Friday is due");
    assert!(!habit.due(2), "Saturday is not due");
    assert!(!habit.due(3), "Sunday is not due");
    assert!(habit.due(4), "Monday is due");
}
#[test]
fn clara_bw_today_controls_fit() {
    let screen = ScreenBuilder::new("hb-today")
        .top_bar("Habits")
        .grid(1, false, [("done-0", "Read"), ("skip-0", "Skip Read")])
        .build();
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    for action in ["done-0", "skip-0"] {
        let rect = layout.rect_of_action(action_id(action)).expect("control");
        assert!(rect.height >= CLARA_BW_METRICS.touch_target_minimum());
    }
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
}
