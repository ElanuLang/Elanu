use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

const WORKSTATION: &str = include_str!("../../examples/workstation_single_line.elnu");
const PARAMETERIZED_WORKSTATION: &str =
    include_str!("../../examples/workstation_parameterized_actions.elnu");

fn runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("workstation source should parse and check");
    Runtime::from_program(&program).expect("workstation runtime should initialize")
}

#[test]
fn scalar_workstation_composes_current_kernel_semantics() {
    let mut runtime = runtime(WORKSTATION);

    assert_eq!(
        runtime.value("sku").unwrap(),
        Value::String("SKU-001".to_string())
    );
    assert_eq!(runtime.value("subtotal").unwrap(), Value::Float(25.0));
    assert_eq!(runtime.value("tax").unwrap(), Value::Float(2.0));
    assert_eq!(runtime.value("total").unwrap(), Value::Float(27.0));
    assert_eq!(runtime.value("projectedInventory").unwrap(), Value::Int(8));

    runtime.run_action("increaseQuantity").unwrap();

    assert_eq!(runtime.value("quantity").unwrap(), Value::Int(3));
    assert_eq!(runtime.value("subtotal").unwrap(), Value::Float(37.5));
    assert_eq!(runtime.value("tax").unwrap(), Value::Float(3.0));
    assert_eq!(runtime.value("total").unwrap(), Value::Float(40.5));
    assert_eq!(runtime.value("projectedInventory").unwrap(), Value::Int(7));

    runtime.run_action("checkout").unwrap();

    assert_eq!(runtime.value("inventory").unwrap(), Value::Int(7));
    assert_eq!(runtime.value("checkedOut").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("projectedInventory").unwrap(), Value::Int(7));

    let error = runtime
        .run_action("increaseQuantity")
        .expect_err("checked-out invoice should reject edits");
    assert!(error.message.contains("already checked out"));
    assert_eq!(runtime.value("quantity").unwrap(), Value::Int(3));
    assert_eq!(runtime.value("inventory").unwrap(), Value::Int(7));
}

#[test]
fn failed_nested_checkout_rolls_back_the_whole_workstation_transition() {
    let source = format!(
        "{WORKSTATION}\n\naction attemptOversoldCheckout {{\n    quantity = inventory + 1\n    checkout()\n}}\n"
    );
    let mut runtime = runtime(&source);

    let error = runtime
        .run_action("attemptOversoldCheckout")
        .expect_err("oversold checkout should fail");

    assert!(error.message.contains("Insufficient inventory"));
    assert_eq!(runtime.value("quantity").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("inventory").unwrap(), Value::Int(10));
    assert_eq!(runtime.value("checkedOut").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("subtotal").unwrap(), Value::Float(25.0));
    assert_eq!(runtime.value("total").unwrap(), Value::Float(27.0));
}

#[test]
fn parameterized_workstation_composes_explicit_authority_and_transaction_local_reads() {
    let mut runtime = runtime(PARAMETERIZED_WORKSTATION);

    assert_eq!(runtime.value("quantityA").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("quantityB").unwrap(), Value::Int(10));

    runtime.run_action("update").unwrap();

    assert_eq!(runtime.value("quantityA").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("quantityB").unwrap(), Value::Int(7));
}
