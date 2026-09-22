use elanu_compiler::{
    check_source, parse_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn parses_state_model_and_modeled_state_variables() {
    let source = r#"
state model Invoice {
    state quantity = 1
    state limit = 10
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state invoiceB: Invoice
"#;

    let program = parse_source(source).expect("state-model surface should parse");
    assert_eq!(program.state_models.len(), 1);
    assert_eq!(program.state_models[0].name, "Invoice");
    assert_eq!(program.declarations.len(), 2);

    check_source(source).expect("state-model surface should lower and check");
}

#[test]
fn same_named_members_have_distinct_live_identity_and_dependencies() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
    state limit = 10
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state invoiceB: Invoice

derived aRemaining = invoiceA.remainingCapacity
derived bRemaining = invoiceB.remainingCapacity

action changeA {
    invoiceA.quantity += 1
}
"#,
    );

    assert_eq!(runtime.value("aRemaining").unwrap(), Value::Int(9));
    assert_eq!(runtime.value("bRemaining").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("aRemaining"), Some(1));
    assert_eq!(runtime.derived_evaluations("bRemaining"), Some(1));

    runtime.run_action("changeA").unwrap();

    assert_eq!(runtime.value("aRemaining").unwrap(), Value::Int(8));
    assert_eq!(runtime.value("bRemaining").unwrap(), Value::Int(9));
    assert_eq!(runtime.derived_evaluations("aRemaining"), Some(2));
    assert_eq!(runtime.derived_evaluations("bRemaining"), Some(1));
}

#[test]
fn exact_member_authority_targets_only_the_selected_member() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
    state limit = 10
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state invoiceB: Invoice

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state invoiceA.quantity)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(1));
}

#[test]
fn whole_modeled_state_authority_uses_existing_action_semantics() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
    state limit = 10
    state checkedOut = false
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state invoiceB: Invoice

derived aCheckedOut = invoiceA.checkedOut
derived bCheckedOut = invoiceB.checkedOut

action checkout(state invoice: Invoice) {
    if invoice.quantity > invoice.limit {
        fail "quantity exceeds limit"
    }
    invoice.checkedOut = true
}

action demo {
    checkout(state invoiceA)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("aCheckedOut").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("bCheckedOut").unwrap(), Value::Bool(false));
}

#[test]
fn modeled_state_value_parameter_reads_stored_and_derived_properties() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 2
    state limit = 10
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state observed = 0

action inspect(invoice: Invoice, state output: Int) {
    output = invoice.quantity + invoice.remainingCapacity
}

action demo {
    inspect(invoiceA, state observed)
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(10));
}

#[test]
fn modeled_state_value_parameter_does_not_gain_writable_authority() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice

action bad(invoice: Invoice) {
    invoice.quantity = 2
}

action demo {
    bad(invoiceA)
}
"#;

    let errors = check_source(source).expect_err("value model parameter must remain read-only");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("cannot assign to value parameter")),
        "expected value-parameter mutation diagnostic, got {errors:#?}"
    );
}

#[test]
fn model_local_derived_cannot_capture_unrelated_live_state() {
    let source = r#"
state globalTaxRate = 0.08

state model Invoice {
    state subtotal = 100.0
    derived tax = subtotal * globalTaxRate
}

state invoiceA: Invoice
"#;

    let errors = check_source(source).expect_err("model-local derived must remain closed");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("state-model member") && error.message.contains("globalTaxRate")
        }),
        "expected model-locality diagnostic, got {errors:#?}"
    );
}

#[test]
fn one_failed_action_rolls_back_writes_across_two_modeled_state_variables() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice

derived aQuantity = invoiceA.quantity
derived bQuantity = invoiceB.quantity

action increment(state invoice: Invoice) {
    invoice.quantity += 1
}

action demo {
    increment(state invoiceA)
    increment(state invoiceB)
    fail "rollback"
}
"#,
    );

    assert!(runtime.run_action("demo").is_err());
    assert_eq!(runtime.value("aQuantity").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("bQuantity").unwrap(), Value::Int(1));
}

#[test]
fn whole_and_member_authority_alias_the_same_semantic_state_identity() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 5
}

state invoiceA: Invoice
derived observed = invoiceA.quantity

action confuse(state invoice: Invoice, state quantity: Int) {
    invoice.quantity = 1
    quantity = 2
}

action demo {
    confuse(state invoiceA, state invoiceA.quantity)
}
"#,
    );

    runtime.run_action("demo").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
}

#[test]
fn transaction_local_model_derived_reads_see_staged_member_values() {
    let mut runtime = checked_runtime(
        r#"
state model Invoice {
    state quantity = 1
    state limit = 10
    derived remainingCapacity = limit - quantity
}

state invoiceA: Invoice
state seen = 0
derived finalRemaining = invoiceA.remainingCapacity

action demo {
    invoiceA.quantity = 4
    seen = invoiceA.remainingCapacity
    invoiceA.quantity = 7
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("seen").unwrap(), Value::Int(6));
    assert_eq!(runtime.value("finalRemaining").unwrap(), Value::Int(3));
}

#[test]
fn whole_state_assignment_is_explicitly_deferred() {
    let source = r#"
state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice

action bad {
    invoiceB = invoiceA
}
"#;

    let errors = check_source(source).expect_err("whole state assignment is deferred");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("whole-state assignment")),
        "expected whole-state-assignment diagnostic, got {errors:#?}"
    );
}

#[test]
fn scalar_typed_state_still_requires_initializer() {
    let source = r#"
state quantity: Int
"#;

    let errors = check_source(source).expect_err("ordinary typed state still requires initializer");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("requires an initializer")),
        "expected missing initializer diagnostic, got {errors:#?}"
    );
}
