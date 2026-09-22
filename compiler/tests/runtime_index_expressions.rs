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
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice
state selectedLine = 0
state observed = 0

derived selectedQuantity = invoice.lines[selectedLine].quantity

action setQuantity(state target: Int, value: Int) {
    target = value
}

action setup {
    create LineItem in invoice as first {
        through first.quantity = 2
        insert first into invoice.lines
    }
    create LineItem in invoice as second {
        through second.quantity = 3
        insert second into invoice.lines
    }
}

action selectSecondAndRead {
    selectedLine = 1
    observed = invoice.lines[selectedLine].quantity
}

action selectSecondAndMutate {
    selectedLine = 1
    through invoice.lines[selectedLine].quantity = 7
    observed = invoice.lines[selectedLine].quantity
}

action selectSecondAndGrant {
    selectedLine = 1
    setQuantity(state through invoice.lines[selectedLine].quantity, 9)
    observed = invoice.lines[selectedLine].quantity
}

action selectNegativeAndRead {
    selectedLine = 0 - 1
    observed = invoice.lines[selectedLine].quantity
}

action selectMissingAndGrant {
    selectedLine = 9
    setQuantity(state through invoice.lines[selectedLine].quantity, 11)
}
"#;

#[test]
fn runtime_index_reads_staged_int_state() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("selectSecondAndRead")
        .expect("runtime index should observe staged selectedLine");
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(3));
}

#[test]
fn runtime_index_direct_mutation_targets_selected_child() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("selectSecondAndMutate")
        .expect("runtime index mutation should commit");
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(7));
}

#[test]
fn runtime_index_state_through_grant_targets_selected_child() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    runtime
        .run_action("selectSecondAndGrant")
        .expect("runtime indexed authority grant should commit");
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(9));
    assert_eq!(runtime.value("selectedQuantity").unwrap(), Value::Int(9));
}

#[test]
fn negative_and_out_of_bounds_runtime_indices_fail_transactionally() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    let negative = runtime
        .run_action("selectNegativeAndRead")
        .expect_err("negative runtime index should fail");
    assert!(negative.message.contains("cannot be negative"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));

    let missing = runtime
        .run_action("selectMissingAndGrant")
        .expect_err("out-of-bounds runtime grant should fail");
    assert!(missing.message.contains("out of bounds"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
}

#[test]
fn runtime_index_expression_must_be_int() {
    let errors = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state invoice: Invoice
derived invalid = invoice.lines[1.0].quantity
"#,
    )
    .expect_err("Float index should be rejected statically");

    assert!(errors.iter().any(|error| error
        .message
        .contains("sequence index expression must be Int")));
}
