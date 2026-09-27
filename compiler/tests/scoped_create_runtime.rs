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
    state quantity = 2
    state unitPrice = 5.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice
state observedQuantity = 0
state observedTotal = 0.0

action commitScopedCreate {
    create LineItem in invoice as line {
        through line.quantity = 3
        observedQuantity = line.quantity
        observedTotal = line.lineTotal
    }
}

action rollbackScopedCreate {
    create LineItem in invoice as line {
        through line.quantity = 7
        observedQuantity = line.quantity
        observedTotal = line.lineTotal
        fail "rollback scoped child"
    }
}
"#;

#[test]
fn scoped_create_body_observes_and_mutates_the_fresh_identity() {
    let mut runtime = runtime(SOURCE);

    runtime
        .run_action("commitScopedCreate")
        .expect("scoped creation should commit");

    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(3));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(15.0));
}

#[test]
fn scoped_create_failure_rolls_back_body_writes_and_fresh_identity() {
    let mut runtime = runtime(SOURCE);

    let error = runtime
        .run_action("rollbackScopedCreate")
        .expect_err("failure inside scoped create should abort the outer action");

    assert!(error.message.contains("rollback scoped child"));
    assert_eq!(runtime.value("observedQuantity").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observedTotal").unwrap(), Value::Float(0.0));

    // This test intentionally probes the bootstrap representation to ensure the
    // failed transaction did not publish the freshly created child.
    assert!(runtime
        .value("__elanu_sm$__elanu_dynamic$LineItem0$quantity")
        .is_err());
}

#[test]
fn scoped_create_designation_does_not_escape_its_lexical_block() {
    let source = r#"
state model LineItem {
    state quantity = 2
}
state model Invoice {
    state count = 0
}
state invoice: Invoice
state observed = 0

action invalidEscape {
    create LineItem in invoice as line {
        observed = line.quantity
    }
    observed = line.quantity
}
"#;

    let errors = check_source(source).expect_err("scoped designation should not escape");
    assert!(errors.iter().any(|error| {
        error.message.contains("unknown modeled-state root 'line'")
            || error.message.contains("unknown value 'line.quantity'")
    }));
}
