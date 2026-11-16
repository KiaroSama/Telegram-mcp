use super::*;

#[test]
fn repeated_short_rendered_text_keeps_window_storage_bounded_and_preserves_fragments() {
    let mut memory = Memory::default();
    for _ in 0..20_000 {
        memory.note_value("work", &json!(42), &json!({"message":"short @peer_canary"}));
    }
    let account = &memory.accounts["work"];
    assert_eq!(account.total, 0);
    assert!(
        account.batches.is_empty(),
        "Empty passage batches cannot consume unbounded memory"
    );
    assert!(
        !memory
            .find("work", &json!({"message":"@peer_canary"}))
            .is_empty()
    );
}
