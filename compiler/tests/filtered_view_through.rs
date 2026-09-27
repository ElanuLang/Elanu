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

action setup {
    itemA.quantity = 2
    itemB.quantity = 3
    itemC.quantity = 0
    invoice.lines = [live itemA, live itemB, live itemC]
}

action deactivateFirstThenReadCurrentFirst {
    through invoice.activeLines[0].quantity = 0
    observed = invoice.activeLines[0].quantity
}

action activateCThenMutateSelected {
    itemC.quantity = 5
    selected = 2
    through invoice.activeLines[selected].quantity += 2
    observed = itemC.quantity
}

action replaceMembershipThenMutate {
    invoice.lines = [live itemC, live itemA]
    itemC.quantity = 4
    through invoice.activeLines[0].quantity = 7
    observed = itemC.quantity
}

action duplicateThenMutateSecondOccurrence {
    invoice.lines = [live itemA, live itemA, live itemB]
    itemA.quantity = 2
    selected = 1
    through invoice.activeLines[selected].quantity = 0
    observed = invoice.activeLines[0].quantity
}

action outOfBoundsAfterStagedWrites {
    selected = 9
    itemA.note = 8
    through invoice.activeLines[selected].quantity = 1
}

action negativeAfterStagedWrites {
    selected = 0 - 1
    itemA.note = 9
    through invoice.activeLines[selected].quantity = 1
}

action mutateThenFail {
    through invoice.activeLines[0].quantity = 11
    itemA.note = 6
    fail "rollback"
}
"#;

#[test]
fn mutation_resolves_exact_child_before_filter_membership_changes() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("deactivateFirstThenReadCurrentFirst")
        .expect("filtered mutation should commit");

    assert_eq!(
        runtime.value("__elanu_sm$itemA$quantity").unwrap(),
        Value::Int(0)
    );
    assert_eq!(
        runtime.value("__elanu_sm$itemB$quantity").unwrap(),
        Value::Int(3)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
}

#[test]
fn staged_predicate_changes_are_visible_before_mutation_target_resolution() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("activateCThenMutateSelected")
        .expect("staged predicate change should affect current filtered view");

    assert_eq!(runtime.value("selected").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("__elanu_sm$itemC$quantity").unwrap(),
        Value::Int(7)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn staged_membership_is_visible_before_mutation_target_resolution() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("replaceMembershipThenMutate")
        .expect("staged membership should affect current filtered view");

    assert_eq!(
        runtime.value("__elanu_sm$itemC$quantity").unwrap(),
        Value::Int(7)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn duplicate_occurrence_selection_does_not_create_occurrence_identity() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("duplicateThenMutateSecondOccurrence")
        .expect("duplicate occurrence should resolve the designated child");

    assert_eq!(
        runtime.value("__elanu_sm$itemA$quantity").unwrap(),
        Value::Int(0)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
}

#[test]
fn invalid_filtered_mutation_indices_fail_and_roll_back_prior_writes() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    let missing = runtime
        .run_action("outOfBoundsAfterStagedWrites")
        .expect_err("out-of-bounds filtered mutation should fail");
    assert!(missing.message.contains("out of bounds"));
    assert_eq!(runtime.value("selected").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("__elanu_sm$itemA$note").unwrap(),
        Value::Int(0)
    );

    let negative = runtime
        .run_action("negativeAfterStagedWrites")
        .expect_err("negative filtered mutation should fail");
    assert!(negative.message.contains("cannot be negative"));
    assert_eq!(runtime.value("selected").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("__elanu_sm$itemA$note").unwrap(),
        Value::Int(0)
    );
}

#[test]
fn later_failure_rolls_back_filtered_target_mutation() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    assert!(runtime.run_action("mutateThenFail").is_err());
    assert_eq!(
        runtime.value("__elanu_sm$itemA$quantity").unwrap(),
        Value::Int(2)
    );
    assert_eq!(
        runtime.value("__elanu_sm$itemA$note").unwrap(),
        Value::Int(0)
    );
}

#[test]
fn filtered_through_cannot_mutate_derived_child_member() {
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

action invalid {
    through invoice.activeLines[0].doubled = 4
}
"#,
    )
    .expect_err("derived child member must remain non-writable");

    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot mutate derived member 'doubled'")));
    assert!(errors
        .iter()
        .all(|error| !error.message.contains("__elanu_")));
}

#[test]
fn state_through_filtered_selection_is_accepted() {
    check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line { line.quantity > 0 }
}
state invoice: Invoice

action setQuantity(state target: Int) {
    target = 4
}

action mutateSelected {
    setQuantity(state through invoice.activeLines[0].quantity)
}
"#,
    )
    .expect("filtered-view selection may now grant exact writable state authority");
}
