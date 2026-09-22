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
    state selectedIndex = 1
    state selectedKey = ""
    state tasks: [live Task] = []

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
}

state a: Task
state b: Task
state c: Task
state workstation: Workstation
state selectedTask: live Task = live c
state observed = ""

derived positionCode = workstation.selectedPositionCode

action setupUnique {
    a.key = "A"
    a.title = "A"
    b.key = "B"
    b.title = "B"
    c.key = "C"
    c.title = "C"
    b.active = false
    workstation.tasks = [live a, live b, live c]
    workstation.selectedIndex = 1
    selectedTask = workstation.visibleTasks[workstation.selectedIndex]
    workstation.selectedKey = selectedTask.key
}

action revealEarlier {
    b.active = true
}

action mutatePersistedSelection {
    through selectedTask.title = "C selected"
    observed = c.title
}

action removeAtStalePosition {
    remove workstation.visibleTasks[workstation.selectedIndex]
    observed = selectedTask.title
}

action removeAtBusinessKeyPosition {
    remove workstation.visibleTasks[0 - workstation.selectedPositionCode - 1]
    observed = selectedTask.title
}

action setupDuplicateKeys {
    a.key = "same"
    a.title = "A"
    c.key = "same"
    c.title = "C"
    workstation.tasks = [live a, live c]
    workstation.selectedIndex = 1
    selectedTask = workstation.tasks[workstation.selectedIndex]
    workstation.selectedKey = selectedTask.key
}
"#;

#[test]
fn persisted_designation_already_handles_child_level_work_after_view_position_changes() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupUnique").unwrap();
    runtime.run_action("revealEarlier").unwrap();
    runtime.run_action("mutatePersistedSelection").unwrap();

    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C selected".into())
    );
}

#[test]
fn stale_positional_structural_edit_does_not_follow_persisted_child_identity() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupUnique").unwrap();
    runtime.run_action("revealEarlier").unwrap();

    // selectedIndex is still 1, so after B enters [A, B, C], positional removal
    // removes B. The persisted designation still identifies C.
    runtime.run_action("removeAtStalePosition").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );

    // The current business-key reconstruction now sees C at visible position 1.
    assert_eq!(runtime.value("positionCode").unwrap(), Value::Int(-2));
}

#[test]
fn business_key_can_recover_position_but_is_not_child_identity() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupUnique").unwrap();
    assert_eq!(runtime.value("positionCode").unwrap(), Value::Int(-2));

    runtime.run_action("revealEarlier").unwrap();
    assert_eq!(runtime.value("positionCode").unwrap(), Value::Int(-3));

    runtime.run_action("removeAtBusinessKeyPosition").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
    assert_eq!(runtime.value("positionCode").unwrap(), Value::Int(2));

    let mut duplicate_runtime = make_runtime(SOURCE);
    duplicate_runtime.run_action("setupDuplicateKeys").unwrap();

    // selectedTask designates C at structural position 1, but the copied business
    // key matches A first. The reconstruction therefore reports position 0.
    assert_eq!(
        duplicate_runtime.value("positionCode").unwrap(),
        Value::Int(-1)
    );
}
