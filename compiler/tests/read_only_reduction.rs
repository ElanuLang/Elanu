use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn model_local_reduction_handles_empty_and_two_child_subtotals() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 1.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
    derived subtotal = reduce lines from 0.0 as (total, line) {
        total + line.lineTotal
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived observed = invoice.subtotal

action populate {
    itemA.quantity = 2
    itemA.unitPrice = 3.0
    itemB.quantity = 1
    itemB.unitPrice = 7.0
    invoice.lines = [live itemA, live itemB]
}
"#,
    );

    assert_eq!(runtime.value("observed").unwrap(), Value::Float(0.0));
    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Float(13.0));
}

#[test]
fn reduction_dependencies_follow_only_current_children_and_replace_on_removal() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
    derived amount = quantity * 10
}

state model Invoice {
    state lines: [live LineItem] = []
    derived subtotal = reduce lines from 0 as (total, line) {
        total + line.amount
    }
}

state itemA: LineItem
state itemB: LineItem
state itemC: LineItem
state invoice: Invoice

derived observed = invoice.subtotal

action populate {
    invoice.lines = [live itemA, live itemB]
}

action changeA {
    itemA.quantity += 1
}

action changeB {
    itemB.quantity += 1
}

action changeC {
    itemC.quantity += 1
}

action removeB {
    invoice.lines = [live itemA]
}
"#,
    );

    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(20));
    assert_eq!(runtime.derived_evaluations("observed"), Some(1));

    runtime.run_action("changeC").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(20));
    assert_eq!(runtime.derived_evaluations("observed"), Some(1));

    runtime.run_action("changeA").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(30));
    assert_eq!(runtime.derived_evaluations("observed"), Some(2));

    runtime.run_action("removeB").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(20));
    assert_eq!(runtime.derived_evaluations("observed"), Some(3));

    runtime.run_action("changeB").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(20));
    assert_eq!(runtime.derived_evaluations("observed"), Some(3));
}

#[test]
fn newly_added_child_becomes_dependency_after_recomputation() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + line.quantity
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived observed = invoice.total

action start {
    invoice.lines = [live itemA]
}

action addB {
    invoice.lines = [live itemA, live itemB]
}

action changeB {
    itemB.quantity += 1
}
"#,
    );

    runtime.run_action("start").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    runtime.run_action("addB").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
    assert_eq!(runtime.derived_evaluations("observed"), Some(2));

    runtime.run_action("changeB").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
    assert_eq!(runtime.derived_evaluations("observed"), Some(3));
}

#[test]
fn reduction_observes_sequence_order_and_duplicate_multiplicity() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived digits = reduce lines from 0 as (total, line) {
        total * 10 + line.quantity
    }
    derived sum = reduce lines from 0 as (total, line) {
        total + line.quantity
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived digits = invoice.digits
derived sum = invoice.sum

action populate {
    itemA.quantity = 1
    itemB.quantity = 2
    invoice.lines = [live itemA, live itemB]
}

action reorder {
    invoice.lines = [live itemB, live itemA]
}

action duplicateA {
    invoice.lines = [live itemA, live itemA]
}
"#,
    );

    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("digits").unwrap(), Value::Int(12));
    assert_eq!(runtime.value("sum").unwrap(), Value::Int(3));

    runtime.run_action("reorder").unwrap();
    assert_eq!(runtime.value("digits").unwrap(), Value::Int(21));

    runtime.run_action("duplicateA").unwrap();
    assert_eq!(runtime.value("digits").unwrap(), Value::Int(11));
    assert_eq!(runtime.value("sum").unwrap(), Value::Int(2));
}

#[test]
fn transaction_local_reduction_sees_staged_membership_and_child_writes_and_rolls_back() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 1.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
    derived subtotal = reduce lines from 0.0 as (total, line) {
        total + line.lineTotal
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice
state seen = 0.0

derived observed = invoice.subtotal

action stage {
    invoice.lines = [live itemA]
    itemA.unitPrice = 4.0
    seen = invoice.subtotal
}

action failStage {
    invoice.lines = [live itemB]
    itemB.unitPrice = 9.0
    seen = invoice.subtotal
    fail "rollback"
}
"#,
    );

    runtime.run_action("stage").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Float(4.0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Float(4.0));

    assert!(runtime.run_action("failStage").is_err());
    assert_eq!(runtime.value("seen").unwrap(), Value::Float(4.0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Float(4.0));
}

#[test]
fn empty_reduction_does_not_evaluate_the_step_expression() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state numerator = 4
    state divisor = 0
    derived risky = numerator / divisor
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + line.risky
    }
}

state itemA: LineItem
state invoice: Invoice

derived observed = invoice.total
"#,
    );

    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
}

#[test]
fn failed_child_derived_read_is_not_cached_as_successful_reduction() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state numerator = 4
    state divisor = 0
    derived risky = numerator / divisor
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + line.risky
    }
}

state itemA: LineItem
state invoice: Invoice

derived observed = invoice.total

action populate {
    invoice.lines = [live itemA]
}

action fix {
    itemA.divisor = 2
}
"#,
    );

    runtime.run_action("populate").unwrap();
    assert!(runtime.value("observed").is_err());
    runtime.run_action("fix").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
}

#[test]
fn model_local_reduction_preserves_rooted_locality() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + itemA.quantity
    }
}

state itemA: LineItem
state invoice: Invoice
"#,
    )
    .expect_err("ambient live root must not be captured by model-local reduction");

    assert!(errors
        .iter()
        .any(|error| error.message.contains("rooted model-local reduction")));
}

#[test]
fn reduction_step_is_expression_only_and_has_no_writable_authority() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        line.quantity = 2
    }
}

state itemA: LineItem
state invoice: Invoice
"#,
    )
    .expect_err("assignment inside reduction step must not become legal");

    assert!(errors
        .iter()
        .any(|error| error.message.contains("invalid step expression")));
}
