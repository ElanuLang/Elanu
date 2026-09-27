use elanu_compiler::{
    check_source, check_source_with_runtime_models,
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
}

state invoice: Invoice
state observedGranted = 0
state observedCurrent = 0
state calleeEntered = false

action setQuantity(state target: Int, value: Int) {
    calleeEntered = true
    target = value
}

action setFloat(state target: Float, value: Float) {
    target = value
}

action setThenFail(state target: Int, value: Int) {
    target = value
    fail "reject forwarded authority"
}

action removeThenWriteGranted(state target: Int) {
    remove invoice.lines[0]
    target = 11
    observedGranted = target
    observedCurrent = invoice.lines[0].quantity
}

action setup {
    create LineItem in invoice as first {
        through first.quantity = 2
        insert first into invoice.lines
    }
    create LineItem in invoice as second {
        through second.quantity = 5
        insert second into invoice.lines
    }
}

action forwardFirst {
    setQuantity(state through invoice.lines[0].quantity, 7)
    observedCurrent = invoice.lines[0].quantity
}

action removeThenForward {
    remove invoice.lines[0]
    setQuantity(state through invoice.lines[0].quantity, 9)
    observedCurrent = invoice.lines[0].quantity
}

action grantThenRestructure {
    removeThenWriteGranted(state through invoice.lines[0].quantity)
}

action forwardThenFail {
    setThenFail(state through invoice.lines[0].quantity, 8)
}

action forwardMissing {
    setQuantity(state through invoice.lines[9].quantity, 12)
}
"#;

fn line_targets(runtime: &mut Runtime) -> Vec<String> {
    let Value::Sequence { targets, .. } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should use runtime-sized sequence storage");
    };
    targets
}

#[test]
fn forwards_exact_authority_to_dynamically_inserted_child() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("forwardFirst")
        .expect("runtime-selected authority should bind");

    assert_eq!(runtime.value("observedCurrent").unwrap(), Value::Int(7));
    assert_eq!(runtime.value("calleeEntered").unwrap(), Value::Bool(true));
}

#[test]
fn grant_resolution_uses_membership_current_at_call_time() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let before = line_targets(&mut runtime);

    runtime
        .run_action("removeThenForward")
        .expect("grant should follow membership after staged removal");

    assert_eq!(line_targets(&mut runtime), vec![before[1].clone()]);
    assert_eq!(runtime.value("observedCurrent").unwrap(), Value::Int(9));
}

#[test]
fn granted_authority_does_not_retarget_after_callee_restructures_sequence() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let before = line_targets(&mut runtime);

    runtime
        .run_action("grantThenRestructure")
        .expect("grant should remain bound after structural edit");

    assert_eq!(line_targets(&mut runtime), vec![before[1].clone()]);
    assert_eq!(runtime.value("observedGranted").unwrap(), Value::Int(11));
    assert_eq!(runtime.value("observedCurrent").unwrap(), Value::Int(5));
}

#[test]
fn forwarded_authority_shares_outer_transaction_and_rolls_back() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let before = line_targets(&mut runtime);

    let error = runtime
        .run_action("forwardThenFail")
        .expect_err("nested failure should abort shared transaction");
    assert!(error.message.contains("reject forwarded authority"));
    assert_eq!(line_targets(&mut runtime), before);

    runtime
        .run_action("forwardFirst")
        .expect("original target should remain writable after rollback");
    assert_eq!(runtime.value("observedCurrent").unwrap(), Value::Int(7));
}

#[test]
fn out_of_bounds_grant_fails_before_entering_callee() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    assert_eq!(runtime.value("calleeEntered").unwrap(), Value::Bool(false));

    let error = runtime
        .run_action("forwardMissing")
        .expect_err("out-of-bounds grant should fail during binding");

    assert!(error.message.contains("out of bounds"));
    assert_eq!(runtime.value("calleeEntered").unwrap(), Value::Bool(false));
}

#[test]
fn derived_member_cannot_be_forwarded_as_writable_authority() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
    derived doubled = quantity * 2
}
state model Invoice {
    state lines: [live LineItem] = []
}
state item: LineItem
state invoice: Invoice

action mutate(state target: Int) {
    target = 3
}

action seed {
    invoice.lines = [live item]
}

action invalid {
    mutate(state through invoice.lines[0].doubled)
}
"#,
    )
    .expect_err("derived member should not grant writable authority");

    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot grant derived member 'doubled'")));
}

#[test]
fn forwarded_authority_preserves_exact_state_type() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state item: LineItem
state invoice: Invoice

action needsFloat(state target: Float) {
    target = 3.0
}

action seed {
    invoice.lines = [live item]
}

action invalid {
    needsFloat(state through invoice.lines[0].quantity)
}
"#,
    )
    .expect_err("state authority must not widen Int to Float");

    assert!(errors.iter().any(|error| {
        error.message.contains("writable state argument")
            && error.message.contains("Int")
            && error.message.contains("Float")
    }));
}
