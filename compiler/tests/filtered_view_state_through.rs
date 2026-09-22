use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model LineItem {
    state quantity = 0
    state note = 0
    derived doubled = quantity * 2
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity > 0
    }
}

state itemA: LineItem
state itemB: LineItem
state itemC: LineItem
state invoice: Invoice
state selected = 0
state observed = 0

action setQuantity(state target: Int, value: Int) {
    target = value
}

action forwardQuantity(state target: Int, value: Int) {
    setQuantity(state target, value)
}

action setup {
    itemA.quantity = 2
    itemB.quantity = 3
    itemC.quantity = 0
    invoice.lines = [live itemA, live itemB, live itemC]
}

action grantThenDeactivateFirst {
    setQuantity(state through invoice.activeLines[0].quantity, 0)
    observed = invoice.activeLines[0].quantity
}

action activateCThenGrantSelected {
    itemC.quantity = 5
    selected = 2
    setQuantity(state through invoice.activeLines[selected].quantity, 7)
    observed = itemC.quantity
}

action replaceMembershipThenGrant {
    invoice.lines = [live itemC, live itemA]
    itemC.quantity = 4
    setQuantity(state through invoice.activeLines[0].quantity, 8)
    observed = itemC.quantity
}

action duplicateGrant {
    invoice.lines = [live itemA, live itemA, live itemB]
    itemA.quantity = 2
    selected = 1
    setQuantity(state through invoice.activeLines[selected].quantity, 0)
    observed = invoice.activeLines[0].quantity
}

action forwardGrant {
    forwardQuantity(state through invoice.activeLines[0].quantity, 6)
    observed = itemA.quantity
}

action outOfBoundsGrant {
    selected = 9
    itemA.note = 8
    setQuantity(state through invoice.activeLines[selected].quantity, 1)
}

action negativeGrant {
    selected = 0 - 1
    itemA.note = 9
    setQuantity(state through invoice.activeLines[selected].quantity, 1)
}

action grantThenFail {
    setQuantity(state through invoice.activeLines[0].quantity, 11)
    itemA.note = 6
    fail "rollback"
}
"#;

#[test]
fn grant_resolves_exact_child_before_filter_membership_changes() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("grantThenDeactivateFirst")
        .expect("filtered state-through grant should commit");

    assert_eq!(
        runtime.value("__meld_sm$itemA$quantity").unwrap(),
        Value::Int(0)
    );
    assert_eq!(
        runtime.value("__meld_sm$itemB$quantity").unwrap(),
        Value::Int(3)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
}

#[test]
fn staged_predicate_changes_are_visible_before_grant_resolution() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("activateCThenGrantSelected")
        .expect("staged predicate change should affect grant-time view");

    assert_eq!(runtime.value("selected").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("__meld_sm$itemC$quantity").unwrap(),
        Value::Int(7)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn staged_membership_is_visible_before_grant_resolution() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("replaceMembershipThenGrant")
        .expect("staged membership should affect grant-time view");

    assert_eq!(
        runtime.value("__meld_sm$itemC$quantity").unwrap(),
        Value::Int(8)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(8));
}

#[test]
fn duplicate_positions_grant_the_same_underlying_child_state() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("duplicateGrant")
        .expect("duplicate position should grant exact child state");

    assert_eq!(
        runtime.value("__meld_sm$itemA$quantity").unwrap(),
        Value::Int(0)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
}

#[test]
fn filtered_grant_can_be_forwarded_without_rebinding_to_view_position() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("forwardGrant")
        .expect("filtered grant should forward exact state authority");

    assert_eq!(
        runtime.value("__meld_sm$itemA$quantity").unwrap(),
        Value::Int(6)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(6));
}

#[test]
fn invalid_filtered_grant_indices_fail_and_roll_back_prior_writes() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    let missing = runtime
        .run_action("outOfBoundsGrant")
        .expect_err("out-of-bounds grant should fail");
    assert!(missing.message.contains("out of bounds"));
    assert_eq!(runtime.value("selected").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("__meld_sm$itemA$note").unwrap(),
        Value::Int(0)
    );

    let negative = runtime
        .run_action("negativeGrant")
        .expect_err("negative grant should fail");
    assert!(negative.message.contains("cannot be negative"));
    assert_eq!(runtime.value("selected").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("__meld_sm$itemA$note").unwrap(),
        Value::Int(0)
    );
}

#[test]
fn later_failure_rolls_back_filtered_grant_mutation() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    assert!(runtime.run_action("grantThenFail").is_err());

    assert_eq!(
        runtime.value("__meld_sm$itemA$quantity").unwrap(),
        Value::Int(2)
    );
    assert_eq!(
        runtime.value("__meld_sm$itemA$note").unwrap(),
        Value::Int(0)
    );
}

#[test]
fn filtered_grant_cannot_target_derived_child_member() {
    let errors = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
    derived doubled = quantity * 2
}
state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line { line.quantity > 0 }
}
state invoice: Invoice

action consume(state target: Int) {
    target = 4
}

action invalid {
    consume(state through invoice.activeLines[0].doubled)
}
"#,
    )
    .expect_err("derived child member must remain non-grantable");

    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot grant derived member 'doubled' as writable state")));
    assert!(errors
        .iter()
        .all(|error| !error.message.contains("__meld_")));
}
