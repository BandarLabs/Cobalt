#[cfg(test)]
mod interrupted_capture {
use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{DisplayMetrics, CLARA_BW_METRICS};
fn fixture() -> ReadLater {
    ReadLater {server:"https://bag.example".into(),depth:50,entries_origin:Some("https://bag.example".into()),entries:(0..30).map(|i|Entry{id:i+1,title:format!("Article {i}"),site:"example.org".into(),reading_time:3,position:0,content:if i==0 {String::new()} else {"Cached reading body.".into()},starred:false,archived:false}).collect(),..ReadLater::default()}
}
fn latest(commands:&[Command])->&kobo_sdk::Screen { commands.iter().rev().find_map(|c|if let Command::SetScreen(s)=c{Some(s)}else{None}).unwrap() }
fn fetch(commands:&[Command])->TaskId { commands.iter().find_map(|c|if let Command::Spawn{task,work:Task::Fetch{..}}=c{Some(*task)}else{None}).unwrap() }
#[test]
fn native_review_snapshots() {
    for (name, scale) in [("default", kobo_ui::TextScale::Default), ("largest", kobo_ui::TextScale::Largest)] {
        let metrics = DisplayMetrics {text_scale: scale, ..CLARA_BW_METRICS};
        let capture = |runner: &AppRunner<ReadLater>, _commands: &[Command], label: &str| {
            let mut context = runner.context();
            runner.app().show(&mut context);
            crate::audit_capture(latest(context.commands()), &context, &format!("{name}-{label}"));
        };
        let mut app = fixture();
        app.session = Some(session::Session {server:"https://bag.example".into(),client_id:"id".into(),client_secret:"synthetic".into(),refresh_token:"synthetic".into()});
        let mut runner = AppRunner::with_metrics(app, metrics);
        let task = fetch(&runner.action(action_id("entry-0")));
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unauthorized));
        let refresh = runner.app().task.unwrap().0;
        runner.action(ActionId::BACK);
        runner.action(action_id("queue-next"));
        runner.task_outcome(refresh,TaskOutcome::Completed(br#"{"access_token":"replacement","refresh_token":"rolled"}"#.to_vec()));
        let commands=runner.device_result(DeviceResult::Done);
        capture(&runner, &commands, "dismissed-refresh");

        let mut app = fixture();
        app.entries[1].archived=true;
        let mut runner = AppRunner::with_metrics(app, metrics);
        let task=fetch(&runner.action(action_id("sync")));
        runner.action(action_id("archive-tab"));
        let commands=runner.task_outcome(task,TaskOutcome::Completed(br#"{"items":[{"id":1,"title":"Unread","is_archived":0}]}"#.to_vec()));
        capture(&runner, &commands, "late-unread");
    }
}

}
