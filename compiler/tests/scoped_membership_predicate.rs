use elanu_compiler::check_source_with_runtime_models;

#[test]
fn scoped_designation_cannot_be_membership_predicate_operand() {
    let source = r#"
state model Task {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state task: Task
state workstation: Workstation
state selectedTask: live Task = live task
state observed = false

action bad {
    with selectedTask as captured {
        observed = captured is in workstation.tasks
    }
}
"#;

    let diagnostics = check_source_with_runtime_models(source)
        .expect_err("scoped designation membership should remain outside the language surface");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(messages.contains("left operand of 'is in' must be a persistent live designation"));
    assert!(!messages.contains("__meld_"));
}
