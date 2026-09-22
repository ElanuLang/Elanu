use elanu_compiler::{
    check_source, parse_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn parses_and_checks_minimum_ordered_sequence_surface() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived firstQuantity = order[0].quantity

action demo {
    order = [live invoiceB, live invoiceA]
    through order[0].quantity += 1
}
"#;

    parse_source(source).expect("ordered-sequence surface should parse");
    check_source(source).expect("ordered-sequence surface should lower and check");
}

#[test]
fn reorder_moves_indexed_read_and_dynamic_dependency() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived firstQuantity = order[0].quantity

action setA {
    invoiceA.quantity = 3
}

action setB {
    invoiceB.quantity = 8
}

action bumpA {
    invoiceA.quantity += 1
}

action bumpB {
    invoiceB.quantity += 1
}

action swap {
    order = [live invoiceB, live invoiceA]
}
"#,
    );

    runtime.run_action("setA").unwrap();
    runtime.run_action("setB").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(3));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(1));

    runtime.run_action("bumpB").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(3));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(1));

    runtime.run_action("swap").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(2));

    runtime.run_action("bumpA").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(2));

    runtime.run_action("bumpB").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(10));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(3));
}

#[test]
fn staged_reorder_controls_later_through_write() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action demo {
    order = [live invoiceB, live invoiceA]
    through order[0].quantity += 4
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(5));
}

#[test]
fn nested_action_reorder_is_visible_to_later_indexed_through() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action swap {
    order = [live invoiceB, live invoiceA]
}

action demo {
    swap()
    through order[0].quantity += 4
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(5));
}

#[test]
fn transaction_local_derived_read_follows_staged_reorder() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived firstQuantity = order[0].quantity
derived aQuantity = invoiceA.quantity

action demo {
    invoiceB.quantity = 7
    order = [live invoiceB, live invoiceA]
    if firstQuantity == 7 {
        invoiceA.quantity = 4
    }
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(7));
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(4));
}

#[test]
fn sequence_reorder_and_target_write_roll_back_together() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived firstQuantity = order[0].quantity
derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action demo {
    order = [live invoiceB, live invoiceA]
    through order[0].quantity = 9
    fail "abort"
}
"#,
    );

    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(1));
    assert!(runtime.run_action("demo").is_err());
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(1));
}

#[test]
fn equivalent_whole_sequence_replacement_does_not_invalidate() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived firstQuantity = order[0].quantity

action sameAgain {
    order = [live invoiceA, live invoiceB]
}
"#,
    );

    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(1));
    runtime.run_action("sameAgain").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("firstQuantity"), Some(1));
}

#[test]
fn variable_length_replacement_preserves_designated_identity() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived firstQuantity = order[0].quantity

action setB {
    invoiceB.quantity = 7
}

action keepOnlyB {
    order = [live invoiceB]
}
"#,
    );

    runtime.run_action("setB").unwrap();
    runtime.run_action("keepOnlyB").unwrap();
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(7));
}

#[test]
fn indexed_member_authority_flows_to_action_parameter() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action increment(state target: Int) {
    target += 1
}

action demo {
    order = [live invoiceB, live invoiceA]
    increment(state through order[0].quantity)
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(2));
}

#[test]
fn whole_indexed_target_authority_uses_existing_model_action_semantics() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
    state checkedOut = false
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived aChecked = invoiceA.checkedOut
derived bChecked = invoiceB.checkedOut

action checkout(state invoice: Invoice) {
    invoice.checkedOut = true
}

action demo {
    order = [live invoiceB, live invoiceA]
    checkout(state through order[0])
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("aChecked").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("bChecked").unwrap(), Value::Bool(true));
}

#[test]
fn indexed_authority_is_exact_at_grant_time_even_if_callee_reorders() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action incrementAndSwap(state target: Int) {
    target += 1
    order = [live invoiceB, live invoiceA]
}

action demo {
    incrementAndSwap(state through order[0].quantity)
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(1));
}

#[test]
fn direct_indexed_mutation_is_rejected_without_through() {
    let errors = check_source(
        r#"
state model Invoice {
    state quantity = 1
}
state invoiceA: Invoice
state order: [live Invoice] = [live invoiceA]
action bad {
    order[0].quantity = 4
}
"#,
    )
    .expect_err("sequence positions must not become writable state slots");

    assert!(errors
        .iter()
        .any(|error| error.message.contains("not independently writable state")));
}

#[test]
fn indexed_state_grant_is_rejected_without_through() {
    let errors = check_source(
        r#"
state model Invoice {
    state quantity = 1
}
state invoiceA: Invoice
state order: [live Invoice] = [live invoiceA]
action increment(state target: Int) {
    target += 1
}
action bad {
    increment(state order[0].quantity)
}
"#,
    )
    .expect_err("indexed target authority must remain explicit");

    assert!(errors
        .iter()
        .any(|error| error.message.contains("requires 'state through'")));
}

#[test]
fn whole_sequence_grant_cannot_leak_private_string_representation() {
    let errors = check_source(
        r#"
state model Invoice {
    state quantity = 1
}
state invoiceA: Invoice
state order: [live Invoice] = [live invoiceA]
action corrupt(state target: String) {
    target = "not a sequence"
}
action bad {
    corrupt(state order)
}
"#,
    )
    .expect_err("bootstrap scalar representation must not become source-level sequence typing");

    assert!(errors.iter().any(|error| error
        .message
        .contains("whole ordered-sequence authority cannot be passed")));
}

#[test]
fn known_out_of_bounds_variant_is_rejected() {
    let errors = check_source(
        r#"
state model Invoice {
    state quantity = 1
}
state invoiceA: Invoice
state order: [live Invoice] = [live invoiceA]
derived firstQuantity = order[0].quantity
action empty {
    order = []
}
"#,
    )
    .expect_err("constant indexing must be valid for all known bootstrap sequence values");

    assert!(errors
        .iter()
        .any(|error| error.message.contains("not valid for every known value")));
}

#[test]
fn literal_target_must_match_sequence_model_and_availability() {
    let errors = check_source(
        r#"
state model Invoice {
    state quantity = 1
}
state model Customer {
    state quantity = 1
}
state invoiceA: Invoice
state customerA: Customer
state order: [live Invoice] = [live customerA]
"#,
    )
    .expect_err("sequence targets must come from the declared live model");

    assert!(errors.iter().any(|error| error
        .message
        .contains("not an earlier declared live Invoice target")));
}

#[test]
fn sequence_surface_does_not_rewrite_brackets_inside_strings_or_comments() {
    check_source(
        r#"
state text = "[live invoiceA] and order[0]"
// [live Invoice] order[0]
derived echoed = text
"#,
    )
    .expect("brackets inside strings/comments must remain ordinary source text");
}
