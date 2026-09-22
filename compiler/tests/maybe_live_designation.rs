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

    derived taskCount = reduce tasks from 0 as (count, task) {
        count + 1
    }
}

state workstation: Workstation
state hasSelection = false
state selectedTask: maybe live Task = none
state observedTitle = ""
state marker = 0

derived taskCount = workstation.taskCount

action addAndSelect {
    create Task in workstation as task {
        through task.title = "A"
        insert task into workstation.tasks
    }
    selectedTask = workstation.tasks[0]
    hasSelection = true
    observedTitle = selectedTask.title
}

action clearSelection {
    selectedTask = none
    hasSelection = false
}

action clearWithoutRemovingMembership {
    selectedTask = none
    hasSelection = false
    observedTitle = workstation.tasks[0].title
}

action removeSelectedButKeepDesignation {
    remove selectedTask from workstation.tasks
    observedTitle = selectedTask.title
}

action guardedReadWhenAbsent {
    if hasSelection {
        observedTitle = selectedTask.title
    }
}

action absentReadAfterWrite {
    marker = 1
    observedTitle = selectedTask.title
}

action absentMutationAfterWrite {
    marker = 2
    through selectedTask.title = "must not mutate"
}

action absentRemovalAfterWrite {
    marker = 3
    remove selectedTask from workstation.tasks
}

action selectThenFail {
    selectedTask = workstation.tasks[0]
    hasSelection = true
    fail "rollback optional selection"
}

action clearThenFail {
    selectedTask = none
    hasSelection = false
    fail "rollback optional clear"
}
"#;

#[test]
fn maybe_live_state_can_begin_absent_without_a_dummy_target() {
    let mut runtime = make_runtime(SOURCE);

    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(0));
    runtime.run_action("guardedReadWhenAbsent").unwrap();
}

#[test]
fn maybe_live_state_can_select_a_dynamic_child_and_clear_back_to_none() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("addAndSelect").unwrap();
    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(1));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("A".into())
    );

    runtime
        .run_action("clearWithoutRemovingMembership")
        .unwrap();
    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(1));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn membership_removal_does_not_implicitly_clear_a_present_maybe_live_designation() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("addAndSelect").unwrap();
    runtime
        .run_action("removeSelectedButKeepDesignation")
        .unwrap();

    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(true));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn absent_member_read_fails_and_rolls_back_prior_writes() {
    let mut runtime = make_runtime(SOURCE);

    let error = runtime
        .run_action("absentReadAfterWrite")
        .expect_err("absent live read should fail");
    assert!(error.message.contains("no target"));
    assert!(!error.message.contains("__meld_"));
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
}

#[test]
fn absent_through_mutation_fails_and_rolls_back_prior_writes() {
    let mut runtime = make_runtime(SOURCE);

    let error = runtime
        .run_action("absentMutationAfterWrite")
        .expect_err("absent live mutation should fail");
    assert!(error.message.contains("no target"));
    assert!(!error.message.contains("__meld_"));
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
}

#[test]
fn absent_designation_directed_remove_fails_and_rolls_back_prior_writes() {
    let mut runtime = make_runtime(SOURCE);

    let error = runtime
        .run_action("absentRemovalAfterWrite")
        .expect_err("absent live structural removal should fail");
    assert!(error.message.contains("no target"));
    assert!(!error.message.contains("__meld_"));
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
}

#[test]
fn absent_present_transitions_share_outer_transaction_rollback() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addAndSelect").unwrap();

    assert!(runtime.run_action("clearThenFail").is_err());
    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(true));
    runtime
        .run_action("removeSelectedButKeepDesignation")
        .expect("failed clear must restore selected child identity");

    let mut absent = make_runtime(SOURCE);
    absent.run_action("addAndSelect").unwrap();
    absent.run_action("clearSelection").unwrap();
    assert!(absent.run_action("selectThenFail").is_err());
    let error = absent
        .run_action("absentReadAfterWrite")
        .expect_err("failed selection must restore absence");
    assert!(error.message.contains("no target"));
}

#[test]
fn plain_live_state_remains_non_absent() {
    let diagnostics = check_source_with_runtime_models(
        r#"
state model Task {
    state title = ""
}

state selectedTask: live Task = none
"#,
    )
    .expect_err("plain live T should not accept none");

    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("live Task")));
    assert!(!diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("__meld_")));
}

#[test]
fn absent_state_through_grant_fails_before_entering_callee() {
    let source = r#"
state model Task {
    state title = ""
}

state candidate: Task
state selectedTask: maybe live Task = none
state entered = false

action setTitle(state title: String) {
    entered = true
    title = "changed"
}

action callThroughAbsent {
    setTitle(state through selectedTask.title)
}
"#;

    let mut runtime = make_runtime(source);
    let error = runtime
        .run_action("callThroughAbsent")
        .expect_err("absent authority grant should fail before callee");
    assert!(error.message.contains("no target"));
    assert!(!error.message.contains("__meld_"));
    assert_eq!(runtime.value("entered").unwrap(), Value::Bool(false));
}
