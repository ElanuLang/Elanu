use super::*;
use crate::check_source_with_runtime_models;

// Contract test: lowering may change representation, but it must preserve the
// designation model and whether absence is allowed for persistent designation state.
#[test]
fn runtime_preserves_persistent_designation_model_and_optionality() {
    let checked = check_source_with_runtime_models(
        r#"
state model Task {
    state title = ""
}

state task: Task
state required: live Task = live task
state optional: maybe live Task = none
"#,
    )
    .expect("designation metadata source should check");

    let runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");

    let required = runtime
        .runtime_designations
        .get("__elanu_live$required")
        .expect("plain live designation metadata should survive lowering");
    assert_eq!(required.model_name, "Task");
    assert!(!required.allows_none);

    let optional = runtime
        .runtime_designations
        .get("__elanu_live$optional")
        .expect("maybe-live designation metadata should survive lowering");
    assert_eq!(optional.model_name, "Task");
    assert!(optional.allows_none);
}
