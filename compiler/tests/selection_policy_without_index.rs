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
    state search = ""

    derived visibleTasks = filter tasks as task {
        task.active and task.title contains search ignoring case
    }
}

state workstation: Workstation
state selectedTask: maybe live Task = none
state observedTitle = ""
state membershipObserved = false
state presenceObserved = false
state stagedCounter = 0

action seedAndSelectSecond {
    create Task in workstation as first {
        through first.title = "Alpha"
        insert first into workstation.tasks
    }
    create Task in workstation as second {
        through second.title = "Bravo"
        insert second into workstation.tasks
    }
    create Task in workstation as third {
        through third.title = "Charlie"
        insert third into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[1]
}

action hideEarlierAndUseSelection {
    through workstation.tasks[0].active = false
    membershipObserved = selectedTask is in workstation.visibleTasks
    presenceObserved = selectedTask is present
    observedTitle = selectedTask.title
}

action searchForBravoAndReconcile {
    workstation.search = "BRAVO"
    if selectedTask is present {
        if selectedTask is in workstation.visibleTasks {
            membershipObserved = true
        } else {
            selectedTask = none
            membershipObserved = false
        }
    }
    presenceObserved = selectedTask is present
}

action searchAwayAndReconcile {
    workstation.search = "charlie"
    if selectedTask is present {
        if selectedTask is in workstation.visibleTasks {
            membershipObserved = true
        } else {
            selectedTask = none
            membershipObserved = false
        }
    }
    presenceObserved = selectedTask is present
}

action searchBackAfterClear {
    workstation.search = "bravo"
    membershipObserved = selectedTask is in workstation.visibleTasks
    presenceObserved = selectedTask is present
}

action addDuplicateAndSelect {
    create Task in workstation as earlier {
        through earlier.title = "Able"
        through earlier.active = false
        insert earlier into workstation.tasks
    }
    create Task in workstation as duplicate {
        through duplicate.title = "Echo"
        insert duplicate into workstation.tasks
        insert duplicate into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[0]
    membershipObserved = selectedTask is in workstation.visibleTasks
}

action reconcileDuplicateAfterEarlierEntry {
    through workstation.tasks[0].active = true
    membershipObserved = selectedTask is in workstation.visibleTasks
    observedTitle = selectedTask.title
}

action searchAwayClearThenFail {
    workstation.search = "charlie"
    stagedCounter += 1
    if selectedTask is present {
        if selectedTask is in workstation.visibleTasks {
            membershipObserved = true
        } else {
            selectedTask = none
            membershipObserved = false
        }
    }
    fail "rollback reconciliation"
}

action observeSelectionAfterRollback {
    membershipObserved = selectedTask is in workstation.visibleTasks
    presenceObserved = selectedTask is present
    observedTitle = selectedTask.title
}
"#;

#[test]
fn same_child_selection_survives_position_changes_without_index_reconstruction() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("seedAndSelectSecond").unwrap();

    runtime.run_action("hideEarlierAndUseSelection").unwrap();

    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("presenceObserved").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Bravo".to_string())
    );
}

#[test]
fn search_membership_can_keep_then_clear_the_persisted_selection() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("seedAndSelectSecond").unwrap();

    runtime.run_action("searchForBravoAndReconcile").unwrap();
    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("presenceObserved").unwrap(),
        Value::Bool(true)
    );

    runtime.run_action("searchAwayAndReconcile").unwrap();
    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("presenceObserved").unwrap(),
        Value::Bool(false)
    );

    runtime.run_action("searchBackAfterClear").unwrap();
    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("presenceObserved").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn duplicate_occurrences_do_not_force_occurrence_or_position_reconstruction() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("addDuplicateAndSelect").unwrap();
    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(true)
    );

    runtime
        .run_action("reconcileDuplicateAfterEarlierEntry")
        .unwrap();
    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Echo".to_string())
    );
}

#[test]
fn reconciliation_and_other_staged_state_roll_back_together() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("seedAndSelectSecond").unwrap();

    assert!(runtime.run_action("searchAwayClearThenFail").is_err());
    assert_eq!(runtime.value("stagedCounter").unwrap(), Value::Int(0));

    runtime.run_action("observeSelectionAfterRollback").unwrap();
    assert_eq!(
        runtime.value("membershipObserved").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("presenceObserved").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Bravo".to_string())
    );
}
