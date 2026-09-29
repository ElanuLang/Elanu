use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

/// Established `[T]` sequence semantics should be able to appear at an ordinary
/// immutable action-value boundary.
///
/// This first spike proves only the type boundary. It does not yet require host
/// transport, indexing, traversal, mutation, persistence, or route semantics.
#[test]
fn primitive_sequence_value_parameter_checks() {
    let source = r#"
action accept(route: [Int]) {
}
"#;

    check_source_with_runtime_models(source).expect("ordinary [Int] value parameter should check");
}

/// A host should be able to supply a runtime-sized ordinary sequence to an
/// immutable sequence-valued action parameter.
///
/// This proves transport only. The action does not inspect, index, traverse,
/// mutate, or persist the sequence.
#[test]
fn host_can_supply_runtime_sized_primitive_sequence() {
    let source = r#"
action accept(route: [Int]) {
}
"#;

    let checked = check_source_with_runtime_models(source)
        .expect("ordinary [Int] value parameter should check");

    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");

    runtime
        .run_action_with_values(
            "accept",
            &[Value::ValueSequence(vec![
                Value::Int(0),
                Value::Int(2),
                Value::Int(1),
            ])],
        )
        .expect("runtime-sized [Int] should cross the host action boundary");
}
