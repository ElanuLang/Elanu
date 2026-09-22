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
    state active = true
}

state model Workstation {
    state tasks: [live Task] = []

    derived visibleTasks = filter tasks as task {
        task.active
    }

    derived taskCount = reduce tasks from 0 as (count, task) {
        count + 1
    }
}

state a: Task
state b: Task
state c: Task
state workstation: Workstation
state selectedTask: live Task = live b
state maybeSelected: maybe live Task = none
state observed = ""
state marker = 0

derived taskCount = workstation.taskCount

action writeTitle(state target: String, value: String) {
    target = value
}

action setup {
    a.title = "A"
    b.title = "B"
    c.title = "C"
    a.active = true
    b.active = true
    c.active = true
    workstation.tasks = [live a, live b, live c]
    selectedTask = workstation.tasks[1]
    maybeSelected = none
    observed = ""
    marker = 0
}

action observeSelection {
    observed = selectedTask.title
}

action removeSelectedAndAdvance {
    with selectedTask as removalTarget {
        selectedTask = next selectedTask in workstation.visibleTasks
        remove removalTarget from workstation.visibleTasks
        observed = selectedTask.title
    }
}

action capturePresentMaybe {
    maybeSelected = workstation.tasks[1]
    with maybeSelected as captured {
        observed = captured.title
    }
}

action captureAbsentMaybe {
    marker = 1
    with maybeSelected as captured {
        observed = captured.title
    }
}

action captureIndexed {
    with workstation.tasks[2] as captured {
        observed = captured.title
        through captured.title = "C indexed"
    }
}

action captureRelative {
    with next selectedTask in workstation.tasks as captured {
        observed = captured.title
    }
}

action navigateFromCapturedAnchor {
    with selectedTask as captured {
        selectedTask = next captured in workstation.tasks
        observed = selectedTask.title
    }
}

action captureThenReorderAndRemove {
    with selectedTask as captured {
        selectedTask = next selectedTask in workstation.tasks
        workstation.tasks = [live c, live b, live a]
        remove captured from workstation.tasks
        observed = selectedTask.title
    }
}

action grantThroughCaptured {
    with selectedTask as captured {
        writeTitle(state through captured.title, "B granted")
    }
}

action grantThroughCapturedThenFail {
    with selectedTask as captured {
        writeTitle(state through captured.title, "temporary nested")
        marker = 8
        fail "rollback nested scoped authority"
    }
}

action mutateThenFail {
    with selectedTask as captured {
        through captured.title = "temporary"
        marker = 9
        fail "rollback scoped designation"
    }
}
"#;

#[test]
fn scoped_binding_preserves_old_selection_while_persistent_selection_advances() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("removeSelectedAndAdvance").unwrap();

    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
    runtime.run_action("observeSelection").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn scoped_binding_accepts_present_maybe_and_absent_capture_rolls_back() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("capturePresentMaybe").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );

    runtime.run_action("setup").unwrap();
    let error = runtime
        .run_action("captureAbsentMaybe")
        .expect_err("absent maybe-live capture should fail");
    assert!(error.message.contains("no target"));
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
}

#[test]
fn scoped_binding_captures_indexed_and_relative_designations() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("captureIndexed").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );

    runtime.run_action("setup").unwrap();
    runtime.run_action("captureRelative").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn captured_designation_can_be_a_relative_navigation_anchor() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("navigateFromCapturedAnchor").unwrap();

    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn captured_identity_does_not_retarget_after_later_reorder() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("captureThenReorderAndRemove").unwrap();

    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn captured_designation_supports_through_and_state_through_without_becoming_authority() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("grantThroughCaptured").unwrap();

    runtime.run_action("observeSelection").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B granted".into())
    );
}

#[test]
fn nested_action_through_captured_authority_shares_outer_transaction() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert!(runtime.run_action("grantThroughCapturedThenFail").is_err());

    runtime.run_action("observeSelection").unwrap();
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn later_failure_rolls_back_scoped_designation_body_with_surrounding_action() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert!(runtime.run_action("mutateThenFail").is_err());

    runtime.run_action("observeSelection").unwrap();
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn scoped_designation_is_immutable_and_does_not_escape_its_block() {
    let immutable = r#"
state model Task { state title = "" }
state a: Task
state b: Task
state selected: live Task = live a

action bad {
    with selected as captured {
        captured = live b
    }
}
"#;
    let diagnostics = check_source_with_runtime_models(immutable)
        .expect_err("scoped designation assignment should fail");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("immutable"));
    assert!(!messages.contains("__meld_"));

    let escaped = r#"
state model Task { state title = "" }
state a: Task
state selected: live Task = live a
state observed = ""

action bad {
    with selected as captured {
        observed = captured.title
    }
    observed = captured.title
}
"#;
    let diagnostics = check_source_with_runtime_models(escaped)
        .expect_err("scoped designation should not escape");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("captured"));
    assert!(!messages.contains("__meld_"));
}

#[test]
fn scoped_designation_model_mismatch_is_static_and_does_not_leak_private_names() {
    let source = r#"
state model Task { state title = "" }
state model Other { state title = "" }
state task: Task
state other: Other
state selectedTask: live Task = live task
state selectedOther: live Other = live other

action bad {
    with selectedTask as captured {
        selectedOther = captured
    }
}
"#;
    let diagnostics =
        check_source_with_runtime_models(source).expect_err("wrong designation model should fail");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(messages.contains("live Task"));
    assert!(messages.contains("live Other"));
    assert!(!messages.contains("__meld_"));
}
