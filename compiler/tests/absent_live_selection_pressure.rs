use elanu_compiler::{
    check_source, check_source_with_runtime_models,
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

// Existing-semantics encoding of "no selection": hasSelection is false while
// selectedTask still has to designate some real Task identity.
state dummy: Task
state workstation: Workstation
state hasSelection = false
state selectedTask: live Task = live dummy
state observedTitle = ""

derived taskCount = workstation.taskCount

action readDesignationEvenWhenUnselected {
    observedTitle = selectedTask.title
}

action mutateSentinelWhileUnselected {
    through selectedTask.title = "sentinel is real"
    observedTitle = selectedTask.title
}

action addFirstAndSelect {
    create Task in workstation as task {
        through task.title = "A"
        insert task into workstation.tasks
    }

    selectedTask = workstation.tasks[0]
    hasSelection = true
    observedTitle = selectedTask.title
}

action removeSelectedAndClearFlag {
    if hasSelection {
        remove selectedTask from workstation.tasks
        hasSelection = false
    }
}

action mutateClearedDesignationWithoutGuard {
    through selectedTask.title = "still reachable"
    observedTitle = selectedTask.title
}

action mutateOnlyWhenSelected {
    if hasSelection {
        through selectedTask.title = "guarded mutation"
    }
    observedTitle = selectedTask.title
}

action removeAndClearThenFail {
    remove selectedTask from workstation.tasks
    hasSelection = false
    fail "rollback split selection state"
}
"#;

#[test]
fn bool_plus_live_designation_can_encode_an_initially_empty_selection() {
    let mut runtime = make_runtime(SOURCE);

    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(0));

    // The encoding still requires a real live Task. "No selection" is carried only
    // by the parallel Bool; selectedTask itself remains an ordinary live designation.
    runtime
        .run_action("readDesignationEvenWhenUnselected")
        .unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("".into())
    );

    runtime.run_action("mutateSentinelWhileUnselected").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("sentinel is real".into())
    );
}

#[test]
fn split_state_can_transition_from_no_selection_to_a_real_selected_child() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("addFirstAndSelect").unwrap();

    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(1));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn clearing_the_bool_does_not_make_the_persistent_designation_absent() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addFirstAndSelect").unwrap();
    runtime.run_action("removeSelectedAndClearFlag").unwrap();

    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(0));

    // Membership removal preserves child identity by law. The split encoding therefore
    // leaves selectedTask pointing at A even though application policy says "none".
    // Without an explicit Bool guard, ordinary live-designation mutation still works.
    runtime
        .run_action("mutateClearedDesignationWithoutGuard")
        .unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("still reachable".into())
    );
}

#[test]
fn bool_guarding_can_make_the_split_encoding_operationally_safe_but_is_manual_policy() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addFirstAndSelect").unwrap();
    runtime.run_action("removeSelectedAndClearFlag").unwrap();
    runtime.run_action("mutateOnlyWhenSelected").unwrap();

    // The guard suppresses the mutation, but selectedTask still reads as the removed A.
    // Every operation that intends "current selection" has to remember the parallel Bool.
    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(false));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn split_selection_state_and_structural_edit_roll_back_together() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addFirstAndSelect").unwrap();

    assert!(runtime.run_action("removeAndClearThenFail").is_err());

    assert_eq!(runtime.value("hasSelection").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(1));
    runtime
        .run_action("readDesignationEvenWhenUnselected")
        .unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn scalar_live_state_still_has_no_target_free_initial_state() {
    let diagnostics = check_source(
        r#"
state model Task {
    state title = ""
}

state selectedTask: live Task
"#,
    )
    .expect_err("scalar live state without a target initializer should remain invalid");

    assert!(!diagnostics.is_empty());
}
