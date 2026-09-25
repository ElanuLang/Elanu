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
    state selectedIndex = 1
    state tasks: [live Task] = []

    derived visibleTasks = filter tasks as task {
        task.active
    }
}

state a: Task
state b: Task
state c: Task
state workstation: Workstation
state selectedTask: live Task = live c
state observedTitle = ""

action setup {
    a.title = "A"
    b.title = "B"
    c.title = "C"
    b.active = false
    workstation.tasks = [live a, live b, live c]

    selectedTask = workstation.visibleTasks[workstation.selectedIndex]
    observedTitle = selectedTask.title
}

action revealEarlierRow {
    b.active = true
    observedTitle = selectedTask.title
}

action hideEarlierRow {
    b.active = false
    observedTitle = selectedTask.title
}

action hideSelectedRow {
    c.active = false
    observedTitle = selectedTask.title
}

action removeEarlierOccurrence {
    remove workstation.tasks[0]
    observedTitle = selectedTask.title
}

action removeSelectedOccurrence {
    remove workstation.tasks[2]
    observedTitle = selectedTask.title
}

action mutateSelected {
    through selectedTask.title = "C selected"
    observedTitle = c.title
}

action selectRawRuntime {
    workstation.selectedIndex = 2
    selectedTask = workstation.tasks[workstation.selectedIndex]
    observedTitle = selectedTask.title
}

action selectRawLiteral {
    selectedTask = workstation.tasks[2]
    observedTitle = selectedTask.title
}

action selectDuplicateOccurrence {
    workstation.tasks = [live a, live c, live c]
    selectedTask = workstation.tasks[2]
    through selectedTask.title = "same C"
    observedTitle = c.title
}

action selectOutOfBounds {
    selectedTask = workstation.visibleTasks[9]
}

action selectNegative {
    workstation.selectedIndex = 0 - 1
    selectedTask = workstation.visibleTasks[workstation.selectedIndex]
}

action reselectionThenFail {
    selectedTask = workstation.visibleTasks[0]
    fail "rollback selection identity"
}
"#;

#[test]
fn stored_live_designation_keeps_same_child_when_view_position_changes() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );

    runtime.run_action("revealEarlierRow").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );

    runtime.run_action("hideEarlierRow").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn stored_designation_survives_selected_child_leaving_the_view() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("hideSelectedRow").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn stored_designation_is_independent_of_structural_membership() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("removeEarlierOccurrence").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );

    let mut second_runtime = make_runtime(SOURCE);
    second_runtime.run_action("setup").unwrap();
    second_runtime
        .run_action("removeSelectedOccurrence")
        .unwrap();
    assert_eq!(
        second_runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn stored_live_designation_remains_non_owning_but_supports_explicit_through() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("mutateSelected").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C selected".into())
    );
}

#[test]
fn raw_membership_literal_and_runtime_indices_select_the_same_identity() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("selectRawRuntime").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );

    runtime.run_action("selectRawLiteral").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn duplicate_occurrences_do_not_create_occurrence_identity() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("selectDuplicateOccurrence").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("same C".into())
    );
}

#[test]
fn invalid_whole_designation_indices_are_transactional() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert!(runtime.run_action("selectOutOfBounds").is_err());
    runtime.run_action("revealEarlierRow").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );

    assert!(runtime.run_action("selectNegative").is_err());
    runtime.run_action("hideEarlierRow").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn designation_reselection_rolls_back_with_the_outer_action() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert!(runtime.run_action("reselectionThenFail").is_err());

    runtime.run_action("revealEarlierRow").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn whole_designation_assignment_rejects_the_wrong_model_without_private_name_leakage() {
    let source = r#"
state model Task {
    state title = ""
}

state model Person {
    state name = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state task: Task
state person: Person
state workstation: Workstation
state selectedPerson: live Person = live person

action badSelection {
    workstation.tasks = [live task]
    selectedPerson = workstation.tasks[0]
}
"#;

    let diagnostics =
        check_source_with_runtime_models(source).expect_err("wrong live model should fail");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("live Task"));
    assert!(messages.contains("live Person"));
    assert!(!messages.contains("__elanu_"));
}
