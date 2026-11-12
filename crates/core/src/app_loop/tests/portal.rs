//! Changes asked for in the web UI (`App::on_portal_job`).

use super::*;

// The page is not told a timer it added is saved when the save failed: the answer is an error (500), though the
// timer is set. Once saving works again the same request succeeds.
#[tokio::test(flavor = "current_thread")]
async fn a_portal_timer_that_could_not_be_saved_is_an_error_to_the_page() {
    let dir = TempDir::new("portal-unsaved");
    let Rig { mut app, .. } = rig(&dir, &[]).await;
    let blocker = crate::store::temp_path(&dir.0.join("timers.json"));
    std::fs::create_dir_all(&blocker).unwrap();
    let body = serde_json::json!({"kind": "timer", "seconds": 60, "label": "tea"});
    let (job, answer) = ask(Op::Sched(crate::app::SchedOp::Add(body.clone())));
    app.on_portal_job(job).await;
    assert!(
        matches!(answer.await.unwrap(), Err(Fail::NotSaved(_, false))),
        "the page was told it was saved"
    );
    assert_eq!(
        app.brain.assistant.scheduled().len(),
        1,
        "it is set all the same"
    );
    std::fs::remove_dir(&blocker).unwrap();
    let (job, answer) = ask(Op::Sched(crate::app::SchedOp::Add(body)));
    app.on_portal_job(job).await;
    assert!(answer.await.unwrap().is_ok());
    assert_eq!(timers_on_disk(&dir), 2);
}
