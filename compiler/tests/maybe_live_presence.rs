use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

fn make_runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model Task {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state workstation: Workstation
state selectedTask: maybe live Task = none
state search = "A"
state observed = false
state marker = 0

derived selectionPresent = selectedTask is present

derived selectedMatches =
    selectedTask is present and selectedTask.title contains search

action observePresence {
    observed = selectedTask is present
}

action addAndSelect {
    create Task in workstation as task {
        through task.title = "Alpha"
        insert task into workstation.tasks
    }
    selectedTask = workstation.tasks[0]
    observed = selectedTask is present
}

action clearAndObserve {
    selectedTask = none
    observed = selectedTask is present
}

action selectThenFail {
    selectedTask = workstation.tasks[0]
    fail "rollback presence"
}

action clearThenFail {
    selectedTask = none
    fail "rollback absence"
}

action guardedMutation {
    if selectedTask is present {
        through selectedTask.title = "guarded"
    }
}

action stageSelectAndReadDerived {
    selectedTask = workstation.tasks[0]
    observed = selectedMatches
}

action stageClearAndReadDerived {
    selectedTask = none
    observed = selectedMatches
}

action touchMarker {
    marker += 1
}
"#;

#[test]
fn presence_predicate_observes_absent_and_present_transaction_visible_state() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("observePresence").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(false));
    assert_eq!(
        runtime.value("selectionPresent").unwrap(),
        Value::Bool(false)
    );

    runtime.run_action("addAndSelect").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));
    assert_eq!(
        runtime.value("selectionPresent").unwrap(),
        Value::Bool(true)
    );

    runtime.run_action("clearAndObserve").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(false));
    assert_eq!(
        runtime.value("selectionPresent").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn short_circuit_presence_guard_skips_absent_member_read() {
    let mut runtime = make_runtime(SOURCE);

    // If the RHS were evaluated while absent, this derived read would fail with
    // "live designation has no target". False proves the existing `and` law is enough.
    assert_eq!(
        runtime.value("selectedMatches").unwrap(),
        Value::Bool(false)
    );
    runtime.run_action("guardedMutation").unwrap();
}

#[test]
fn staged_presence_changes_drive_guarded_derived_evaluation_in_the_same_transaction() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addAndSelect").unwrap();
    runtime.run_action("clearAndObserve").unwrap();

    runtime.run_action("stageSelectAndReadDerived").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));

    runtime.run_action("stageClearAndReadDerived").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(false));
}

#[test]
fn presence_and_its_dynamic_dependencies_follow_current_reachability() {
    let mut runtime = make_runtime(SOURCE);

    assert_eq!(
        runtime.value("selectedMatches").unwrap(),
        Value::Bool(false)
    );
    let absent_evaluations = runtime.derived_evaluations("selectedMatches").unwrap();

    // No target is present, so child title must not be an active dependency.
    runtime.run_action("touchMarker").unwrap();
    assert_eq!(
        runtime.value("selectedMatches").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.derived_evaluations("selectedMatches").unwrap(),
        absent_evaluations
    );

    runtime.run_action("addAndSelect").unwrap();
    assert_eq!(runtime.value("selectedMatches").unwrap(), Value::Bool(true));

    runtime.run_action("clearAndObserve").unwrap();
    assert_eq!(
        runtime.value("selectedMatches").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn rollback_restores_presence_state() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addAndSelect").unwrap();

    assert!(runtime.run_action("clearThenFail").is_err());
    assert_eq!(
        runtime.value("selectionPresent").unwrap(),
        Value::Bool(true)
    );

    let mut absent = make_runtime(SOURCE);
    // Put a selectable child in the structure while leaving the designation absent.
    absent.run_action("addAndSelect").unwrap();
    absent.run_action("clearAndObserve").unwrap();
    assert!(absent.run_action("selectThenFail").is_err());
    assert_eq!(
        absent.value("selectionPresent").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn presence_observation_does_not_make_plain_live_or_ordinary_values_optional() {
    let diagnostics = check_source_with_runtime_models(
        r#"
state count = 0

derived invalid = count is present
"#,
    )
    .expect_err("presence should remain live-designation-specific");

    assert!(!diagnostics.is_empty());
    assert!(!diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("__meld_")));
}
