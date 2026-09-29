use elanu_compiler::check_source_with_runtime_models;

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
