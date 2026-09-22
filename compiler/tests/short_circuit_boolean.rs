use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn and_skips_unreachable_filter_dependency_then_acquires_it() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 0
    state unitPrice = 1.0
}

state model Invoice {
    state lines: [live LineItem] = []
    derived actionableLines = filter lines as line {
        line.quantity > 0 and line.unitPrice > 0.0
    }
    derived actionableCount = reduce actionableLines from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice

derived observed = invoice.actionableCount

action setup {
    invoice.lines = [live item]
}

action changePriceWhileShortCircuited {
    item.unitPrice = 2.0
}

action activate {
    item.quantity = 1
}

action changePriceAfterReachable {
    item.unitPrice = 0.0
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    let filter = "__meld_filter_member$invoice$actionableLines";
    assert_eq!(runtime.derived_evaluations(filter), Some(1));

    runtime
        .run_action("changePriceWhileShortCircuited")
        .unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    assert_eq!(runtime.derived_evaluations(filter), Some(1));

    runtime.run_action("activate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter), Some(2));

    runtime.run_action("changePriceAfterReachable").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    assert_eq!(runtime.derived_evaluations(filter), Some(3));
}

#[test]
fn or_skips_unreachable_filter_dependency_then_acquires_it() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 0.0
}

state model Invoice {
    state lines: [live LineItem] = []
    derived visibleLines = filter lines as line {
        line.quantity > 0 or line.unitPrice > 0.0
    }
    derived visibleCount = reduce visibleLines from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice

derived observed = invoice.visibleCount

action setup {
    invoice.lines = [live item]
}

action changePriceWhileShortCircuited {
    item.unitPrice = 2.0
}

action deactivate {
    item.quantity = 0
}

action changePriceAfterReachable {
    item.unitPrice = 0.0
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    let filter = "__meld_filter_member$invoice$visibleLines";
    assert_eq!(runtime.derived_evaluations(filter), Some(1));

    runtime
        .run_action("changePriceWhileShortCircuited")
        .unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter), Some(1));

    runtime.run_action("deactivate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter), Some(2));

    runtime.run_action("changePriceAfterReachable").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    assert_eq!(runtime.derived_evaluations(filter), Some(3));
}

#[test]
fn short_circuit_suppresses_unreached_runtime_failure() {
    let mut runtime = checked_runtime(
        r#"
derived safeAnd = false and (1 / 0 > 0)
derived safeOr = true or (1 / 0 > 0)
"#,
    );

    assert_eq!(runtime.value("safeAnd").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("safeOr").unwrap(), Value::Bool(true));
}

#[test]
fn not_negates_bool_and_composes_with_filter_predicates() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state disabled = false
}

state model Invoice {
    state lines: [live LineItem] = []
    derived enabledLines = filter lines as line {
        not line.disabled
    }
    derived enabledCount = reduce enabledLines from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice

derived observed = invoice.enabledCount

action setup {
    invoice.lines = [live item]
}

action disable {
    item.disabled = true
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    runtime.run_action("disable").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
}

#[test]
fn boolean_precedence_is_not_then_and_then_or() {
    let mut runtime = checked_runtime(
        r#"
derived first = true or false and false
derived second = not false and false
"#,
    );

    assert_eq!(runtime.value("first").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("second").unwrap(), Value::Bool(false));
}

#[test]
fn staged_reachability_is_visible_but_rollback_does_not_replace_committed_dependencies() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 0
    state unitPrice = 1.0
}

state model Invoice {
    state lines: [live LineItem] = []
    derived actionableLines = filter lines as line {
        line.quantity > 0 and line.unitPrice > 0.0
    }
    derived actionableCount = reduce actionableLines from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice
state seen = 0

derived observed = invoice.actionableCount

action setup {
    invoice.lines = [live item]
}

action reachRightThenFail {
    item.quantity = 1
    seen = invoice.actionableCount
    fail "rollback"
}

action changePrice {
    item.unitPrice = 2.0
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    let filter = "__meld_filter_member$invoice$actionableLines";
    assert_eq!(runtime.derived_evaluations(filter), Some(1));

    assert!(runtime.run_action("reachRightThenFail").is_err());
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(0));
    let after_failed_transaction = runtime.derived_evaluations(filter).unwrap();

    runtime.run_action("changePrice").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.derived_evaluations(filter),
        Some(after_failed_transaction)
    );
}

#[test]
fn boolean_operators_require_bool_operands_without_private_name_leakage() {
    for source in [
        "derived bad = 1 and true\n",
        "derived bad = true or 1\n",
        "derived bad = not 1\n",
    ] {
        let errors = check_source(source).expect_err("non-Bool Boolean operand must be rejected");
        let joined = errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Bool"), "unexpected diagnostics: {joined}");
        assert!(!joined.contains("__meld_"), "private name leaked: {joined}");
    }
}
