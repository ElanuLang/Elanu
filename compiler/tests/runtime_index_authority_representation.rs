use elanu_compiler::{check_source_with_runtime_models, runtime::Runtime};

#[test]
fn runtime_indexed_authority_carrier_is_not_program_visible_state() {
    let checked = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state item: LineItem
state invoice: Invoice

action mutate(state target: Int) {
    target = 2
}

action seed {
    invoice.lines = [live item]
}

action forward {
    mutate(state through invoice.lines[0].quantity)
}
"#,
    )
    .expect("runtime-indexed authority source should check");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");

    let snapshot = runtime.snapshot().expect("snapshot should succeed");
    assert!(snapshot
        .iter()
        .all(|entry| !entry.name.starts_with("__elanu_runtime_index_grant$")));
}
