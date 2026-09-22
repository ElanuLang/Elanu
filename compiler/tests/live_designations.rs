use elanu_compiler::{
    check_source, parse_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn parses_and_checks_minimum_live_designation_surface() {
    let source = r#"
state model Invoice {
    state quantity = 1
    state limit = 10
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA
state chooseA = true

derived selected: live Invoice =
    if chooseA {
        live invoiceA
    } else {
        live invoiceB
    }
derived activeQuantity = active.quantity

action increment(state target: Int) {
    target += 1
}

action checkout(state invoice: Invoice) {
    invoice.quantity += 1
}

action demo {
    active = live invoiceB
    through active.quantity += 1
    increment(state through active.quantity)
    checkout(state through selected)
}
"#;

    parse_source(source).expect("live designation surface should parse");
    check_source(source).expect("live designation surface should lower and check");
}

#[test]
fn stored_designation_read_through_follows_reselection() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived activeQuantity = active.quantity

action setA {
    invoiceA.quantity = 4
}

action setB {
    invoiceB.quantity = 9
}

action bumpB {
    invoiceB.quantity += 1
}

action selectB {
    active = live invoiceB
}
"#,
    );

    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(1));

    runtime.run_action("setB").unwrap();
    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(1));

    runtime.run_action("selectB").unwrap();
    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(2));

    runtime.run_action("setA").unwrap();
    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(2));

    runtime.run_action("bumpB").unwrap();
    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(10));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(3));
}

#[test]
fn computed_designation_moves_dynamic_member_dependency() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state chooseA = true

derived selected: live Invoice =
    if chooseA {
        live invoiceA
    } else {
        live invoiceB
    }
derived selectedQuantity = selected.quantity

action setA {
    invoiceA.quantity = 4
}

action setB {
    invoiceB.quantity = 8
}

action bumpB {
    invoiceB.quantity += 1
}

action chooseB {
    chooseA = false
}
"#,
    );

    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(1));

    runtime.run_action("setB").unwrap();
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(1));

    runtime.run_action("chooseB").unwrap();
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(8));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(2));

    runtime.run_action("setA").unwrap();
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(8));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(2));

    runtime.run_action("bumpB").unwrap();
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("selectedQuantity"), Some(3));
}

#[test]
fn same_target_reselection_is_an_invalidation_noop() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived activeQuantity = active.quantity

action selectAAgain {
    active = live invoiceA
}
"#,
    );

    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(1));

    runtime.run_action("selectAAgain").unwrap();

    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("activeQuantity"), Some(1));
}

#[test]
fn through_assignment_mutates_only_the_current_target() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action selectBAndIncrement {
    active = live invoiceB
    through active.quantity += 1
}
"#,
    );

    runtime.run_action("selectBAndIncrement").unwrap();

    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(2));
}

#[test]
fn whole_target_authority_through_designation_uses_existing_action_semantics() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
    state checkedOut = false
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived aCheckedOut = invoiceA.checkedOut
derived bCheckedOut = invoiceB.checkedOut

action checkout(state invoice: Invoice) {
    invoice.checkedOut = true
}

action demo {
    active = live invoiceB
    checkout(state through active)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("aCheckedOut").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("bCheckedOut").unwrap(), Value::Bool(true));
}

#[test]
fn exact_member_authority_through_designation_targets_selected_member() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action increment(state target: Int) {
    target += 1
}

action demo {
    active = live invoiceB
    increment(state through active.quantity)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(2));
}

#[test]
fn target_authority_is_resolved_before_callee_reselection() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity
derived activeQuantity = active.quantity

action mutateAndReselect(state invoice: Invoice) {
    active = live invoiceB
    invoice.quantity += 1
}

action demo {
    mutateAndReselect(state through active)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(1));
}

#[test]
fn transaction_local_reselection_controls_later_read_through() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA
state seen = 0

action demo {
    invoiceB.quantity = 7
    active = live invoiceB
    seen = active.quantity
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(7));
}

#[test]
fn transaction_local_computed_designation_sees_staged_selector_for_read_and_authority() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state chooseA = true

derived selected: live Invoice =
    if chooseA {
        live invoiceA
    } else {
        live invoiceB
    }
state seen = 0
derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action increment(state target: Int) {
    target += 1
}

action demo {
    invoiceB.quantity = 7
    chooseA = false
    seen = selected.quantity
    increment(state through selected.quantity)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("seen").unwrap(), Value::Int(7));
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(8));
}

#[test]
fn rollback_restores_selection_and_selected_target_mutation() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

derived activeQuantity = active.quantity
derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action demo {
    active = live invoiceB
    through active.quantity = 9
    fail "rollback"
}
"#,
    );

    assert!(runtime.run_action("demo").is_err());
    assert_eq!(runtime.value("activeQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(1));
}

#[test]
fn indirect_mutation_without_through_is_rejected() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state active: live Invoice = live invoiceA

action bad {
    active.quantity = 5
}
"#;

    let errors = check_source(source).expect_err("indirect mutation requires through");
    assert!(
        errors
            .iter()
            .any(|error| { error.message.contains("requires 'through active.quantity'") }),
        "expected through diagnostic, got {errors:#?}"
    );
}

#[test]
fn bare_designation_is_not_an_ordinary_value() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state active: live Invoice = live invoiceA

derived bad = active
"#;

    let errors = check_source(source).expect_err("bare designation value is deferred");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("not an ordinary value")
                && error
                    .message
                    .contains("whole target value projection is deferred")
        }),
        "expected designation value diagnostic, got {errors:#?}"
    );
}

#[test]
fn live_action_parameters_remain_deferred() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice

action inspect(live invoice: Invoice) {
    fail "not implemented"
}
"#;

    let errors = parse_source(source).expect_err("live action parameters are deferred");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected action parameter name")),
        "expected live-parameter parse diagnostic, got {errors:#?}"
    );
}

#[test]
fn bootstrap_designation_targets_must_precede_the_designation() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state active: live Invoice = live invoiceB
state invoiceB: Invoice
"#;

    let errors = check_source(source).expect_err("later target is a bootstrap limitation");
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("requires a statically declared modeled-state variable")
                || error.message.contains("declared before designation")
        }),
        "expected bootstrap declaration-order diagnostic, got {errors:#?}"
    );
    assert!(
        errors.iter().any(|error| error.line == 7),
        "expected designation diagnostic on source declaration line, got {errors:#?}"
    );
}
