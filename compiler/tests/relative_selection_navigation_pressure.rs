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
    state key = ""
    state title = ""
    state active = true
}

state model Workstation {
    state tasks: [live Task] = []
    state selectedKey = ""

    derived visibleTasks = filter tasks as task {
        task.active
    }

    // Negative encoding means found: position k is -(k + 1).
    // Non-negative means absent after scanning that many visible rows.
    derived selectedPositionCode = reduce visibleTasks from 0 as (code, task) {
        if code < 0 {
            code
        } else {
            if task.key == selectedKey {
                0 - code - 1
            } else {
                code + 1
            }
        }
    }

    derived visibleCount = reduce visibleTasks from 0 as (count, task) {
        count + 1
    }
}

state workstation: Workstation
state selectedTask: maybe live Task = none
state observedTitle = ""
state observedMembership = false

derived positionCode = workstation.selectedPositionCode

// Unique-key setup: [A, B, C], with B selected.
action setupUnique {
    create Task in workstation as a {
        through a.key = "A"
        through a.title = "A"
        insert a into workstation.tasks
    }
    create Task in workstation as b {
        through b.key = "B"
        through b.title = "B"
        insert b into workstation.tasks
    }
    create Task in workstation as c {
        through c.key = "C"
        through c.title = "C"
        insert c into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[1]
    workstation.selectedKey = selectedTask.key
}

// Framework-style reconstruction: recover a position from a copied business key,
// then use ordinary indexed selection for the next row.
action selectNextByCopiedKey {
    if workstation.selectedPositionCode < 0 {
        if 0 - workstation.selectedPositionCode < workstation.visibleCount {
            selectedTask = workstation.visibleTasks[0 - workstation.selectedPositionCode]
            workstation.selectedKey = selectedTask.key
        }
    }
    observedTitle = selectedTask.title
}

action hideEarlierAndSelectNextByCopiedKey {
    through workstation.tasks[0].active = false
    if workstation.selectedPositionCode < 0 {
        if 0 - workstation.selectedPositionCode < workstation.visibleCount {
            selectedTask = workstation.visibleTasks[0 - workstation.selectedPositionCode]
            workstation.selectedKey = selectedTask.key
        }
    }
    observedTitle = selectedTask.title
}

action mutateSelectedBusinessKey {
    through selectedTask.key = "B2"
    observedMembership = selectedTask is in workstation.visibleTasks
}

// Duplicate business keys: [A(same), B, C(same), D], with C selected.
// Key-based reconstruction finds A, not the designated C.
action setupDuplicateBusinessKeys {
    create Task in workstation as a {
        through a.key = "same"
        through a.title = "A"
        insert a into workstation.tasks
    }
    create Task in workstation as b {
        through b.key = "B"
        through b.title = "B"
        insert b into workstation.tasks
    }
    create Task in workstation as c {
        through c.key = "same"
        through c.title = "C"
        insert c into workstation.tasks
    }
    create Task in workstation as d {
        through d.key = "D"
        through d.title = "D"
        insert d into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[2]
    workstation.selectedKey = selectedTask.key
}

// Duplicate child occurrence: [A, C, C, D]. Selecting either C occurrence persists
// only the child identity C. The stored designation does not retain which occurrence
// supplied it.
action setupDuplicateChildAndSelectFirstOccurrence {
    create Task in workstation as a {
        through a.key = "A"
        through a.title = "A"
        insert a into workstation.tasks
    }
    create Task in workstation as c {
        through c.key = "C"
        through c.title = "C"
        insert c into workstation.tasks
        insert c into workstation.tasks
    }
    create Task in workstation as d {
        through d.key = "D"
        through d.title = "D"
        insert d into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[1]
    workstation.selectedKey = selectedTask.key
}

action setupDuplicateChildAndSelectSecondOccurrence {
    create Task in workstation as a {
        through a.key = "A"
        through a.title = "A"
        insert a into workstation.tasks
    }
    create Task in workstation as c {
        through c.key = "C"
        through c.title = "C"
        insert c into workstation.tasks
        insert c into workstation.tasks
    }
    create Task in workstation as d {
        through d.key = "D"
        through d.title = "D"
        insert d into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[2]
    workstation.selectedKey = selectedTask.key
}

action observeSelectionFacts {
    observedTitle = selectedTask.title
    observedMembership = selectedTask is in workstation.visibleTasks
}
"#;

#[test]
fn unique_business_key_can_reconstruct_relative_navigation_but_application_rebuilds_position() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupUnique").unwrap();
    runtime.run_action("selectNextByCopiedKey").unwrap();

    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn key_reconstruction_tracks_current_view_position_after_earlier_rows_leave() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupUnique").unwrap();
    runtime
        .run_action("hideEarlierAndSelectNextByCopiedKey")
        .unwrap();

    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn mutable_business_key_breaks_reconstruction_while_live_identity_remains_valid() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupUnique").unwrap();
    runtime.run_action("mutateSelectedBusinessKey").unwrap();

    assert_eq!(
        runtime.value("observedMembership").unwrap(),
        Value::Bool(true)
    );
    // The copied key remains "B", but the selected child now has key "B2". The
    // reduction therefore reports absence even though Meld still has the exact child.
    assert_eq!(runtime.value("positionCode").unwrap(), Value::Int(3));
}

#[test]
fn duplicate_business_keys_can_navigate_from_the_wrong_child() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupDuplicateBusinessKeys").unwrap();

    // The persisted designation identifies C, but copied-key reconstruction finds A at
    // position 0. "Next" therefore selects B instead of D.
    runtime.run_action("selectNextByCopiedKey").unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn persistent_child_identity_cannot_distinguish_duplicate_structural_occurrences() {
    let mut first = make_runtime(SOURCE);
    first
        .run_action("setupDuplicateChildAndSelectFirstOccurrence")
        .unwrap();
    first.run_action("observeSelectionFacts").unwrap();

    let mut second = make_runtime(SOURCE);
    second
        .run_action("setupDuplicateChildAndSelectSecondOccurrence")
        .unwrap();
    second.run_action("observeSelectionFacts").unwrap();

    assert_eq!(
        first.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
    assert_eq!(
        second.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
    assert_eq!(
        first.value("observedMembership").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        second.value("observedMembership").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(first.value("positionCode").unwrap(), Value::Int(-2));
    assert_eq!(second.value("positionCode").unwrap(), Value::Int(-2));

    // The two source selections came from different structural occurrences, but after
    // persistence they expose the same child-level facts. "Next after C" is therefore
    // not well-defined from the stored designation alone when C occurs more than once.
}
