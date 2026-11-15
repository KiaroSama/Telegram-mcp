use super::common::*;
fn checklist() -> Value {
    json!({"messages":[{"_":"message","id":20,"media":{"_":"messageMediaToDo","todo":{"_":"todoList","title":{"_":"textWithEntities","text":"Tasks","entities":[]},"others_can_append":true,"others_can_complete":false,"list":[{"_":"todoItem","id":3,"title":{"_":"textWithEntities","text":"A","entities":[]}},{"_":"todoItem","id":9,"title":{"_":"textWithEntities","text":"B","entities":[]}}]},"completions":[{"id":3,"completed_by":{"_":"peerUser","user_id":"42"},"date":123}]}}]})
}
#[tokio::test]
async fn new_checklist_has_stable_one_based_task_ids() {
    let m = Mock::new();
    run(
        &m,
        "send_checklist",
        json!({"chat_id":7,"title":"Tasks","tasks":["A","B"],"others_can_add":true}),
    )
    .await
    .unwrap();
    let p = m.request("messages.sendMedia");
    assert_eq!(p["media"]["todo"]["list"][0]["id"], 1);
    assert_eq!(p["media"]["todo"]["list"][1]["id"], 2);
    assert_eq!(p["media"]["todo"]["others_can_append"], true);
}
#[tokio::test]
async fn appended_task_ids_continue_after_highest() {
    let m = Mock::new().respond("channels.getMessages", checklist());
    run(
        &m,
        "add_checklist_tasks",
        json!({"chat_id":7,"message_id":20,"tasks":["C","D"]}),
    )
    .await
    .unwrap();
    let p = m.request("messages.appendTodoList");
    assert_eq!(p["list"][0]["id"], 10);
    assert_eq!(p["list"][1]["id"], 11);
}
#[tokio::test]
async fn removing_tasks_preserves_other_ids_and_flags() {
    let m = Mock::new().respond("channels.getMessages", checklist());
    run(
        &m,
        "remove_checklist_tasks",
        json!({"chat_id":7,"message_id":20,"task_ids":[3]}),
    )
    .await
    .unwrap();
    let todo = &m.request("messages.editMessage")["media"]["todo"];
    assert_eq!(todo["list"][0]["id"], 9);
    assert_eq!(todo["others_can_append"], true);
    assert_eq!(todo["others_can_complete"], false);
}
#[tokio::test]
async fn unknown_task_ids_do_not_toggle_any_task() {
    let m = Mock::new().respond("channels.getMessages", checklist());
    assert!(
        run(
            &m,
            "set_checklist_tasks_done",
            json!({"chat_id":7,"message_id":20,"done":[12]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.toggleTodoCompleted"), 0);
}
#[tokio::test]
async fn same_task_cannot_be_both_completed_and_incompleted() {
    let m = Mock::new().respond("channels.getMessages", checklist());
    assert!(
        run(
            &m,
            "set_checklist_tasks_done",
            json!({"chat_id":7,"message_id":20,"done":[3],"undone":[3]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.toggleTodoCompleted"), 0);
}
#[tokio::test]
async fn checklist_cannot_be_emptied_by_removing_all_tasks() {
    let m = Mock::new().respond("channels.getMessages", checklist());
    assert!(
        run(
            &m,
            "remove_checklist_tasks",
            json!({"chat_id":7,"message_id":20,"task_ids":[3,9]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.editMessage"), 0);
}
#[tokio::test]
async fn checklist_reads_completion_actor_and_time() {
    let m = Mock::new().respond("channels.getMessages", checklist());
    let r = run(&m, "get_checklist", json!({"chat_id":7,"message_id":20}))
        .await
        .unwrap();
    assert_eq!(r["records"][0]["done"], true);
    assert_eq!(r["records"][0]["completed_by"]["user_id"], "42");
    assert_eq!(r["records"][0]["completed_at"], 123);
    assert_eq!(r["records"][1]["done"], false);
}
#[tokio::test]
async fn published_checklist_task_ceiling_is_enforced() {
    let m = Mock::new().respond(
        "help.getAppConfig",
        json!({"config":{"value":[{"key":"todo_items_max","value":{"value":1.0}}]}}),
    );
    assert!(
        run(
            &m,
            "send_checklist",
            json!({"chat_id":7,"title":"Tasks","tasks":["A","B"]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.sendMedia"), 0);
}
#[tokio::test]
async fn empty_checklist_title_is_refused_before_network() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "send_checklist",
            json!({"chat_id":7,"title":" ","tasks":["A"]})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn empty_checklist_task_is_refused_before_network() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "send_checklist",
            json!({"chat_id":7,"title":"Tasks","tasks":[" "]})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
