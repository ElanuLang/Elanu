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
state selectedTask: live Task = live c
state observed = ""
state observedBool = false

derived taskCount = workstation.taskCount

action setup {
    a.title = "A"
    b.title = "B"
    c.title = "C"
    b.active = false
    workstation.tasks = [live a, live b, live c]
    selectedTask = workstation.visibleTasks[1]
}

action revealEarlierAndRemoveSelectedFromView {
    b.active = true
    remove selectedTask from workstation.visibleTasks
    observed = workstation.tasks[1].title
}

action removeSelectedFromStoredMembership {
    remove selectedTask from workstation.tasks
    observed = selectedTask.title
}

action stagedReorderThenRemoveSelected {
    workstation.tasks = [live c, live a, live b]
    remove selectedTask from workstation.tasks
    observed = workstation.tasks[0].title
}

action reselectThenRemoveSelected {
    selectedTask = workstation.tasks[0]
    remove selectedTask from workstation.tasks
    observed = selectedTask.title
}

action observeFirstTask {
    observed = workstation.tasks[0].title
}

action hideSelectedThenRemoveFromView {
    c.active = false
    remove selectedTask from workstation.visibleTasks
}

action readSelectedActive {
    observedBool = c.active
}

action makeDuplicateThenRemoveSelected {
    workstation.tasks = [live a, live c, live c]
    remove selectedTask from workstation.tasks
}

action removeThenFail {
    remove selectedTask from workstation.tasks
    fail "rollback designation-directed removal"
}
"#;

#[test]
fn designation_directed_remove_from_filtered_view_tracks_child_not_stale_position() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime
        .run_action("revealEarlierAndRemoveSelectedFromView")
        .unwrap();

    // C was selected while the view was [A, C]. B becomes visible first, making
    // the current view [A, B, C]. Removal by selected identity must remove C's
    // exact backing occurrence rather than stale position 1 (B).
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));

    // Membership removal does not destroy or retarget the persisted designation.
    assert!(runtime
        .run_action("removeSelectedFromStoredMembership")
        .is_err());
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
}

#[test]
fn designation_directed_remove_from_stored_membership_uses_current_structure() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime
        .run_action("removeSelectedFromStoredMembership")
        .unwrap();

    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );

    let mut staged = make_runtime(SOURCE);
    staged.run_action("setup").unwrap();
    staged
        .run_action("stagedReorderThenRemoveSelected")
        .unwrap();
    assert_eq!(staged.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(staged.value("observed").unwrap(), Value::String("A".into()));
}

#[test]
fn staged_designation_reselection_controls_the_current_removal_target() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("reselectThenRemoveSelected").unwrap();

    // The staged reselection changes selectedTask from C to A before the remove.
    // The remove therefore consumes A's current occurrence, while selectedTask
    // itself continues to designate A after membership removal.
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("A".into())
    );

    runtime.run_action("observeFirstTask").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn zero_current_matches_fail_and_roll_back_predicate_change() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();

    assert!(runtime
        .run_action("hideSelectedThenRemoveFromView")
        .is_err());
    runtime.run_action("readSelectedActive").unwrap();

    assert_eq!(runtime.value("observedBool").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(3));
}

#[test]
fn duplicate_current_occurrences_are_ambiguous_and_fail_transactionally() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();

    assert!(runtime
        .run_action("makeDuplicateThenRemoveSelected")
        .is_err());
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(3));
}

#[test]
fn later_failure_rolls_designation_directed_structural_edit_back() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();

    assert!(runtime.run_action("removeThenFail").is_err());
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(3));
}

#[test]
fn designation_directed_remove_rejects_wrong_model_without_private_name_leakage() {
    let source = r#"
state model Task {
    state title = ""
}

state model Other {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state task: Task
state other: Other
state workstation: Workstation
state selectedOther: live Other = live other

action bad {
    workstation.tasks = [live task]
    remove selectedOther from workstation.tasks
}
"#;

    let diagnostics =
        check_source_with_runtime_models(source).expect_err("wrong live model should fail");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(messages.contains("live Other"));
    assert!(messages.contains("live Task"));
    assert!(!messages.contains("__elanu_"));
}
