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
}

state workstation: Workstation
state selectedTask: maybe live Task = none
state observed = false
state presentObserved = false

action addTwoAndSelectSecond {
    create Task in workstation as first {
        through first.title = "A"
        insert first into workstation.tasks
    }
    create Task in workstation as second {
        through second.title = "B"
        insert second into workstation.tasks
    }
    selectedTask = workstation.tasks[1]
}

action addDuplicateAndSelect {
    create Task in workstation as duplicate {
        through duplicate.title = "D"
        insert duplicate into workstation.tasks
        insert duplicate into workstation.tasks
    }
    selectedTask = workstation.tasks[0]
    observed = selectedTask is in workstation.tasks
}

action observeStoredMembership {
    observed = selectedTask is in workstation.tasks
}

action observeVisibleMembership {
    observed = selectedTask is in workstation.visibleTasks
    presentObserved = selectedTask is present
}

action hideSelectedAndObserve {
    through selectedTask.active = false
    observed = selectedTask is in workstation.visibleTasks
    presentObserved = selectedTask is present
}

action showSelectedAndObserve {
    through selectedTask.active = true
    observed = selectedTask is in workstation.visibleTasks
}

action hideEarlierAndObserve {
    through workstation.tasks[0].active = false
    observed = selectedTask is in workstation.visibleTasks
}

action clearAndObserve {
    selectedTask = none
    observed = selectedTask is in workstation.tasks
    presentObserved = selectedTask is present
}

action hideSelectedThenFail {
    through selectedTask.active = false
    observed = selectedTask is in workstation.visibleTasks
    fail "rollback view membership"
}
"#;

#[test]
fn persisted_child_membership_is_identity_based_not_position_based() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addTwoAndSelectSecond").unwrap();

    runtime.run_action("observeStoredMembership").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));

    runtime.run_action("observeVisibleMembership").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));

    runtime.run_action("hideEarlierAndObserve").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));
}

#[test]
fn designation_presence_and_current_view_membership_are_distinct_facts() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addTwoAndSelectSecond").unwrap();

    runtime.run_action("hideSelectedAndObserve").unwrap();
    assert_eq!(runtime.value("presentObserved").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(false));

    runtime.run_action("showSelectedAndObserve").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));
}

#[test]
fn absent_maybe_live_designation_is_not_in_any_supported_view() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("clearAndObserve").unwrap();
    assert_eq!(
        runtime.value("presentObserved").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(false));
}

#[test]
fn duplicate_occurrences_still_satisfy_child_level_membership_without_creating_occurrence_identity()
{
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addDuplicateAndSelect").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));
}

#[test]
fn staged_predicate_changes_affect_view_membership_and_failure_rolls_them_back() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addTwoAndSelectSecond").unwrap();

    assert!(runtime.run_action("hideSelectedThenFail").is_err());
    runtime.run_action("observeVisibleMembership").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("presentObserved").unwrap(), Value::Bool(true));
}

#[test]
fn view_membership_relation_remains_live_designation_specific() {
    let diagnostics = check_source_with_runtime_models(
        r#"
state model Task {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state workstation: Workstation
state count = 0

derived invalid = count is in workstation.tasks
"#,
    )
    .expect_err("ordinary values should not gain live-view membership semantics");

    assert!(!diagnostics.is_empty());
    assert!(!diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("__meld_")));
}

#[test]
fn view_membership_requires_the_same_live_model_without_private_name_leakage() {
    let diagnostics = check_source_with_runtime_models(
        r#"
state model Task {
    state title = ""
}

state model Note {
    state text = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state workstation: Workstation
state selectedNote: maybe live Note = none

derived invalid = selectedNote is in workstation.tasks
"#,
    )
    .expect_err("designation and view element models must match");

    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("live Note")
            && diagnostic.message.contains("live Task")));
    assert!(!diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("__meld_")));
}
