use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn state_model_sequence_member_has_distinct_per_root_storage_and_replacement() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state marker = 0
    state lines: [live LineItem] = []
}

state itemA: LineItem
state itemB: LineItem
state invoiceA: Invoice
state invoiceB: Invoice

action populateA {
    invoiceA.lines = [live itemA, live itemB]
}

action populateB {
    invoiceB.lines = [live itemB]
}

action abortA {
    invoiceA.lines = [live itemB]
    fail "abort"
}
"#,
    );

    let a = "__meld_mseq$invoiceA$lines";
    let b = "__meld_mseq$invoiceB$lines";

    assert_eq!(
        runtime.value(a).unwrap(),
        Value::String("__meld_sequence_value$".to_string())
    );
    assert_eq!(
        runtime.value(b).unwrap(),
        Value::String("__meld_sequence_value$".to_string())
    );

    runtime.run_action("populateA").unwrap();
    assert_eq!(
        runtime.value(a).unwrap(),
        Value::String("__meld_sequence_value$itemA|itemB".to_string())
    );
    assert_eq!(
        runtime.value(b).unwrap(),
        Value::String("__meld_sequence_value$".to_string())
    );

    runtime.run_action("populateB").unwrap();
    assert_eq!(
        runtime.value(b).unwrap(),
        Value::String("__meld_sequence_value$itemB".to_string())
    );

    assert!(runtime.run_action("abortA").is_err());
    assert_eq!(
        runtime.value(a).unwrap(),
        Value::String("__meld_sequence_value$itemA|itemB".to_string())
    );
}

#[test]
fn empty_model_sequence_member_declaration_checks_without_new_runtime_law() {
    check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state itemA: LineItem
state invoice: Invoice

action populate {
    invoice.lines = [live itemA]
}
"#,
    )
    .expect("member storage and whole-sequence replacement should compose");
}

#[test]
fn state_model_sequence_member_requires_empty_reusable_default() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = [live itemA]
}

state itemA: LineItem
state invoice: Invoice
"#,
    )
    .expect_err("model defaults must not capture an ambient live root");

    assert!(errors
        .iter()
        .any(|error| error.message.contains("must start as []")));
}

#[test]
fn model_local_sequence_read_waits_for_traversal_pressure() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived leaked = lines
}

state invoice: Invoice
"#,
    )
    .expect_err("whole model-local sequence projection must stay deferred");

    assert!(errors.iter().any(|error| error
        .message
        .contains("read-only traversal/aggregation is the next pressure")));
}

#[test]
fn owner_model_parameter_is_rejected_until_whole_model_authority_includes_sequence_member() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state quantity = 1
    state lines: [live LineItem] = []
}

state invoice: Invoice

action reset(state target: Invoice) {
    target.quantity = 1
}
"#,
    )
    .expect_err("whole-model authority must not silently omit sequence state");

    assert!(errors.iter().any(|error| error
        .message
        .contains("whole-model value/authority must include sequence members")));
}

#[test]
fn owner_model_live_designation_preserves_identity_without_whole_model_projection() {
    check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state quantity = 1
    state lines: [live LineItem] = []
}

state invoice: Invoice
state active: live Invoice = live invoice
"#,
    )
    .expect("identity-only live designation must not require whole-model projection");
}

#[test]
fn source_cannot_project_or_grant_private_sequence_representation() {
    let projection_errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice

derived leaked = invoice.lines
"#,
    )
    .expect_err("whole sequence projection must remain hidden");

    assert!(projection_errors
        .iter()
        .any(|error| error.message.contains("whole-value projection is deferred")));

    let grant_errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice

action corrupt(state target: String) {
    target = "not a sequence"
}

action bad {
    corrupt(state invoice.lines)
}
"#,
    )
    .expect_err("private String representation must not become writable source state");

    assert!(grant_errors.iter().any(|error| error
        .message
        .contains("whole ordered-sequence authority cannot be passed")));
}

#[test]
fn indexed_access_resolves_current_runtime_membership_instead_of_static_variants() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state itemA: LineItem
state invoice: Invoice

derived firstQuantity = invoice.lines[0].quantity

action populate {
    invoice.lines = [live itemA]
}
"#,
    );

    let error = runtime
        .value("firstQuantity")
        .expect_err("empty runtime membership should make index zero out of bounds");
    assert!(error.message.contains("out of bounds"));

    runtime
        .run_action("populate")
        .expect("population should commit");
    assert_eq!(runtime.value("firstQuantity").unwrap(), Value::Int(1));
}
