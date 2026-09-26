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
    state quantity = 1
    state unitPrice = 1.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0.0 as (sum, line) {
        sum + line.lineTotal
    }
}

state invoice: Invoice
state observedQuantity = 0
state observedTotal = 0.0
state observedInvoiceTotal = 0.0
state selectedLine = 0

action setQuantity(state target: Int, value: Int) {
    target = value
}

action selectFirst {
    selectedLine = 0
}

action selectSecond {
    selectedLine = 1
}

action setup {
    create LineItem in invoice as first {
        through first.quantity = 2
        through first.unitPrice = 5.0
        insert first into invoice.lines
    }
    create LineItem in invoice as second {
        through second.quantity = 3
        through second.unitPrice = 7.0
        insert second into invoice.lines
    }
}

action observeFirst {
    observedQuantity = invoice.lines[0].quantity
    observedTotal = invoice.lines[0].lineTotal
    observedInvoiceTotal = invoice.total
}

action mutateFirst {
    through invoice.lines[0].quantity = 4
    observedQuantity = invoice.lines[0].quantity
    observedTotal = invoice.lines[0].lineTotal
    observedInvoiceTotal = invoice.total
}

action removeFirstThenMutateCurrentZero {
    remove invoice.lines[0]
    through invoice.lines[0].quantity = 6
    observedQuantity = invoice.lines[0].quantity
    observedTotal = invoice.lines[0].lineTotal
    observedInvoiceTotal = invoice.total
}

action mutateAndRemoveThenFail {
    through invoice.lines[0].quantity = 9
    remove invoice.lines[0]
    observedQuantity = 99
    fail "rollback runtime index"
}

action readMissing {
    observedQuantity = invoice.lines[9].quantity
}

action writeMissing {
    through invoice.lines[9].quantity = 12
}

action observeSelected {
    observedQuantity = invoice.lines[selectedLine].quantity
    observedTotal = invoice.lines[selectedLine].lineTotal
}

action observeSelectedExpression {
    observedQuantity = invoice.lines[selectedLine + 0].quantity
}

action selectSecondThenMutate {
    selectedLine = 1
    through invoice.lines[selectedLine].quantity = 6
    observedQuantity = invoice.lines[selectedLine].quantity
    observedTotal = invoice.lines[selectedLine].lineTotal
}

action grantSelected {
    setQuantity(state through invoice.lines[selectedLine].quantity, 8)
    observedQuantity = invoice.lines[selectedLine].quantity
}

action selectSecondMutateThenFail {
    selectedLine = 1
    through invoice.lines[selectedLine].quantity = 9
    fail "rollback dynamic index"
}

action selectMissingThenRead {
    selectedLine = 9
    observedQuantity = invoice.lines[selectedLine].quantity
}
"#;

fn line_targets(runtime: &mut Runtime) -> Vec<String> {
    let Value::Sequence { targets, .. } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should be runtime-sized sequence state");
    };
    targets
}

#[test]
fn later_action_reads_dynamically_inserted_member_and_derived_value() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("observeFirst")
        .expect("later indexed reads should resolve runtime membership");

    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(10.0));
    assert_eq!(
        runtime.value("observedInvoiceTotal").unwrap(),
        Value::Float(31.0)
    );
}

#[test]
fn indexed_through_mutates_the_current_designated_child() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let before = line_targets(&mut runtime);

    runtime
        .run_action("mutateFirst")
        .expect("indexed through should mutate current target");

    assert_eq!(line_targets(&mut runtime), before);
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(4));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(20.0));
    assert_eq!(
        runtime.value("observedInvoiceTotal").unwrap(),
        Value::Float(41.0)
    );
}

#[test]
fn indexed_access_re_resolves_position_after_membership_change() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let before = line_targets(&mut runtime);
    assert_eq!(before.len(), 2);

    runtime
        .run_action("removeFirstThenMutateCurrentZero")
        .expect("later indexed operation should follow updated structure");

    let after = line_targets(&mut runtime);
    assert_eq!(after, vec![before[1].clone()]);
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(6));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(42.0));
    assert_eq!(
        runtime.value("observedInvoiceTotal").unwrap(),
        Value::Float(42.0)
    );
}

#[test]
fn failed_action_rolls_back_indexed_child_write_and_membership_edit() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("observeFirst")
        .expect("baseline observation should commit");
    let before = line_targets(&mut runtime);

    let error = runtime
        .run_action("mutateAndRemoveThenFail")
        .expect_err("failure should roll back indexed mutation and removal");
    assert!(error.message.contains("rollback runtime index"));

    assert_eq!(line_targets(&mut runtime), before);
    runtime
        .run_action("observeFirst")
        .expect("rolled-back first target should still be readable");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(10.0));
    assert_eq!(
        runtime.value("observedInvoiceTotal").unwrap(),
        Value::Float(31.0)
    );
}

#[test]
fn out_of_bounds_indexed_read_and_write_fail_without_commit() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("observeFirst")
        .expect("baseline observation should commit");
    let before = line_targets(&mut runtime);

    let read_error = runtime
        .run_action("readMissing")
        .expect_err("out-of-bounds indexed read should fail");
    assert!(read_error.message.contains("out of bounds"));
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(2));

    let write_error = runtime
        .run_action("writeMissing")
        .expect_err("out-of-bounds indexed write should fail");
    assert!(write_error.message.contains("out of bounds"));
    assert_eq!(line_targets(&mut runtime), before);

    runtime
        .run_action("observeFirst")
        .expect("existing membership should remain intact");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(10.0));
}

#[test]
fn runtime_int_index_selects_current_member_for_stored_and_derived_reads() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("observeSelected")
        .expect("zero should select the first structural occurrence");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(10.0));
    runtime
        .run_action("selectSecond")
        .expect("second-position selection should commit");
    runtime
        .run_action("observeSelected")
        .expect("one should select the second structural occurrence");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(3));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(21.0));

    runtime
        .run_action("observeSelectedExpression")
        .expect("an Int expression should be accepted as the index");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(3));
}

#[test]
fn staged_index_state_drives_later_read_and_through_in_the_same_action() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("selectSecondThenMutate")
        .expect("index expression should observe staged state");

    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(6));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(42.0));
}

#[test]
fn runtime_int_index_can_forward_exact_writable_authority() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("selectSecond")
        .expect("second-position selection should commit");

    runtime
        .run_action("grantSelected")
        .expect("state through should resolve the runtime-selected child");

    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(8));
    runtime
        .run_action("selectFirst")
        .expect("first-position selection should remain independent of child identity");
    runtime
        .run_action("observeSelected")
        .expect("first child should remain distinct");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(2));
}

#[test]
fn failed_action_rolls_back_staged_index_and_runtime_selected_child_write() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    let error = runtime
        .run_action("selectSecondMutateThenFail")
        .expect_err("failure should roll back both index state and child mutation");
    assert!(error.message.contains("rollback dynamic index"));

    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
    runtime
        .run_action("selectSecond")
        .expect("second-position selection should commit");
    runtime
        .run_action("observeSelected")
        .expect("second child should retain its pre-failure value");
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(3));
}

#[test]
fn out_of_bounds_runtime_index_rolls_back_the_index_state_change() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    let error = runtime
        .run_action("selectMissingThenRead")
        .expect_err("runtime out-of-bounds selection should fail the action");
    assert!(error.message.contains("out of bounds"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
}

#[test]
fn non_int_index_expression_is_rejected_statically() {
    let errors = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state invoice: Invoice
state selectedLine = 0.5
state observed = 0

action readSelected {
    observed = invoice.lines[selectedLine].quantity
}
"#,
    )
    .expect_err("Float index should not pass semantic checking");

    assert!(errors.iter().any(|error| error
        .message
        .contains("sequence index expression must be Int")));
}
