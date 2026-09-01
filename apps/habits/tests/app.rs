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
        assert!(habit.complete(day));
    }
    assert_eq!(habit.current_streak(4), 3);
    assert!(habit.skip(6));
    assert!(habit.complete(8));
    assert_eq!(habit.current_streak(8), 4);
    assert_eq!(habit.best_streak(8), 4);
}
#[test]
fn persisted_habits_round_trip() {
    let mut habit = Habit::new("Walk".into());
    habit.complete(12);
    assert_eq!(decode(&encode(&[habit.clone()])), vec![habit]);
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
