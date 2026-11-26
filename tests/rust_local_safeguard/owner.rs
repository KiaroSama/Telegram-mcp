use super::*;

#[test]
fn effective_owner_bypass_survives_restart_but_keeps_hard_refusals() {
    let (_directory, state) = fixture();
    let call = json!({"account":"work","chat_id":42,"message_id":1});
    let check =
        |guard: &Safeguard, args: &Value| guard.check("delete_message", args, false, true).unwrap();
    assert!(matches!(
        check(&Safeguard::new(state.clone()), &call),
        Decision::NeedsApproval { .. }
    ));
    state
        .write_json("bypass.json", &json!({"on":true,"until":null,"by":101}))
        .unwrap();
    let guard = Safeguard::new(state.clone());
    assert_eq!(check(&guard, &call), Decision::Allow);
    assert!(matches!(
        check(
            &guard,
            &json!({"file_path":state.root().join("bypass.json")})
        ),
        Decision::Refuse { .. }
    ));
    state
        .write_json("bypass.json", &json!({"on":true,"until":1,"by":101}))
        .unwrap();
    assert!(matches!(
        check(&guard, &call),
        Decision::NeedsApproval { .. }
    ));
    std::fs::write(state.root().join("bypass.json"), b"{").unwrap();
    assert!(matches!(
        check(&guard, &call),
        Decision::NeedsApproval { .. }
    ));
}
