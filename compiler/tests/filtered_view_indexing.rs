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
state selectedActiveLine = 0
state observed = 0

derived selectedQuantity = invoice.activeLines[selectedActiveLine].quantity

action setup {
    itemA.quantity = 2
    itemB.quantity = 0
    itemC.quantity = 5
    invoice.lines = [live itemA, live itemB, live itemA, live itemC]
}

action readSelected {
    observed = invoice.activeLines[selectedActiveLine].quantity
}

action selectThirdAndReadDerived {
    selectedActiveLine = 2
    observed = invoice.activeLines[selectedActiveLine].doubled
}

action activateBThenRead {
    itemB.quantity = 3
    selectedActiveLine = 1
    observed = invoice.activeLines[selectedActiveLine].quantity
}

action replaceMembershipThenRead {
    invoice.lines = [live itemC, live itemA]
    selectedActiveLine = 0
    observed = invoice.activeLines[selectedActiveLine].quantity
}

action selectNegative {
    selectedActiveLine = 0 - 1
    observed = invoice.activeLines[selectedActiveLine].quantity
}

action selectMissing {
    selectedActiveLine = 9
    observed = invoice.activeLines[selectedActiveLine].quantity
}

action changeUnrelatedNote {
    itemC.note = 7
}
"#;

#[test]
fn filtered_view_indexing_is_zero_based_and_preserves_duplicate_order() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("readSelected")
        .expect("first filtered occurrence should be readable");
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));

    runtime
        .run_action("selectThirdAndReadDerived")
        .expect("third filtered occurrence should preserve source order");
    assert_eq!(runtime.value("selectedActiveLine").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(10));
}

#[test]
fn staged_predicate_changes_are_visible_before_filtered_selection() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("activateBThenRead")
        .expect("filtered view should observe staged predicate-driving state");

    assert_eq!(runtime.value("selectedActiveLine").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
}

#[test]
fn staged_membership_replacement_is_visible_before_filtered_selection() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("replaceMembershipThenRead")
        .expect("filtered view should observe staged membership");

    assert_eq!(runtime.value("observed").unwrap(), Value::Int(5));
}

#[test]
fn negative_and_out_of_bounds_filtered_indices_fail_and_roll_back_selector() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(2));

    let negative = runtime
        .run_action("selectNegative")
        .expect_err("negative filtered index should fail");
    assert!(negative.message.contains("cannot be negative"));
    assert_eq!(runtime.value("selectedActiveLine").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));

    let missing = runtime
        .run_action("selectMissing")
        .expect_err("out-of-bounds filtered index should fail");
    assert!(missing.message.contains("out of bounds"));
    assert_eq!(runtime.value("selectedActiveLine").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
}

#[test]
fn filtered_view_index_expression_must_be_int() {
    let errors = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line { line.quantity > 0 }
}
state invoice: Invoice
state selected = 0.5
state observed = 0

action readSelected {
    observed = invoice.activeLines[selected].quantity
}
"#,
    )
    .expect_err("Float filtered selector should be rejected statically");

    assert!(errors.iter().any(|error| error
        .message
        .contains("sequence index expression must be Int")));
}

#[test]
fn filtered_selection_does_not_depend_on_unread_child_facts() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(2));
    let filter_name = "__meld_filter_member$invoice$activeLines";
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(1));

    runtime
        .run_action("changeUnrelatedNote")
        .expect("unrelated child write should commit");

    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(1));
}

#[test]
fn direct_through_selection_from_filtered_view_is_accepted() {
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
state selected = 0

action mutateSelected {
    through invoice.activeLines[selected].quantity = 4
}
"#,
    )
    .expect("direct through mutation through a filtered selection should check");
}
