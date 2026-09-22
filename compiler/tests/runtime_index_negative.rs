use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

// Int typing is static; negative-position validity remains a transactional runtime check.
#[test]
fn negative_runtime_index_fails_and_rolls_back_selector_state() {
    let checked = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state invoice: Invoice
state selectedLine = 0
state observed = 0

action setup {
    create LineItem in invoice as line {
        insert line into invoice.lines
    }
}

action selectNegativeThenRead {
    selectedLine = 0 - 1
    observed = invoice.lines[selectedLine].quantity
}
"#,
    )
    .expect("negative Int index should be a runtime bounds concern, not a type error");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    runtime.run_action("setup").expect("setup should commit");

    let error = runtime
        .run_action("selectNegativeThenRead")
        .expect_err("negative runtime index should fail the action");
    assert!(error.message.contains("cannot be negative"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
}
