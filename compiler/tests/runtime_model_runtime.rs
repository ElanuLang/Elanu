use elanu_compiler::{
    check_source, check_source_with_runtime_models, runtime::Runtime,
    runtime_model_templates::RuntimeModelMemberKind, semantic::ValueType,
};

#[test]
fn checked_source_runtime_carries_model_templates() {
    let source = r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 10.0
    derived lineTotal = quantity * unitPrice
}

state line: LineItem
"#;

    let checked = check_source_with_runtime_models(source).expect("source should check");
    let runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");

    let line_item = runtime
        .runtime_model_template("LineItem")
        .expect("runtime should carry the LineItem template");
    let quantity = line_item.member("quantity").expect("quantity member");

    assert_eq!(quantity.kind, RuntimeModelMemberKind::State);
    assert_eq!(quantity.value_type, ValueType::Int);
}

#[test]
fn legacy_checked_program_runtime_remains_template_free() {
    let source = r#"
state model LineItem {
    state quantity = 1
}

state line: LineItem
"#;

    let checked = check_source(source).expect("source should check");
    let runtime = Runtime::from_program(&checked).expect("runtime should initialize");

    assert!(runtime.runtime_model_template("LineItem").is_none());
}
