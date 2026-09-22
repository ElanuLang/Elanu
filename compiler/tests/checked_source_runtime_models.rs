use elanu_compiler::{check_source, check_source_with_runtime_models};

#[test]
fn checked_source_carries_runtime_model_templates_without_changing_check_source() {
    let source = r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 10.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice
"#;

    let checked = check_source_with_runtime_models(source).expect("source should check");
    assert!(checked.runtime_model_templates.contains_key("LineItem"));
    assert!(checked.runtime_model_templates.contains_key("Invoice"));

    let ordinary = check_source(source).expect("legacy checked-program API should still work");
    assert_eq!(
        format!("{}", checked.program.program),
        format!("{}", ordinary.program)
    );
}
