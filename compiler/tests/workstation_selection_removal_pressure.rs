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
state selectedTask: live Task = live a
state removalTarget: maybe live Task = none
state observed = ""
state observedBool = false
state marker = 0

derived taskCount = workstation.taskCount

action setup {
    a.title = "A"
    b.title = "B"
    c.title = "C"
    workstation.tasks = [live a, live b, live c]
    selectedTask = workstation.visibleTasks[1]
}

// Navigation first loses the old selected identity. Removal therefore consumes C,
// the new selection, rather than B, the row that was selected when the action began.
action advanceThenRemoveSelected {
    selectedTask = next selectedTask in workstation.visibleTasks
    remove selectedTask from workstation.visibleTasks
    observed = selectedTask.title
    observedBool = selectedTask is in workstation.visibleTasks
}

// Removal first preserves selectedTask = B, but B no longer has a current occurrence
// in the view, so relative navigation cannot use it as an anchor. The action fails and
// both the structural removal and marker write roll back.
action removeThenAdvance {
    marker = 1
    remove selectedTask from workstation.visibleTasks
    selectedTask = next selectedTask in workstation.visibleTasks
}

// Current Elanu can encode the intended operation only by introducing another
// persistent application-state designation to remember an operation-local fact.
action advanceAndRemoveWithPersistentScratch {
    removalTarget = selectedTask
    selectedTask = next selectedTask in workstation.visibleTasks
    remove removalTarget from workstation.visibleTasks
    removalTarget = none
    observed = selectedTask.title
    observedBool = selectedTask is in workstation.visibleTasks
}

action observeSelection {
    observed = selectedTask.title
    observedBool = selectedTask is in workstation.visibleTasks
}
"#;

#[test]
fn advancing_before_removal_removes_the_new_selection_not_the_old_one() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("advanceThenRemoveSelected").unwrap();

    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
    assert_eq!(runtime.value("observedBool").unwrap(), Value::Bool(false));
}

#[test]
fn removing_before_navigation_loses_the_required_current_anchor_and_rolls_back() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();

    assert!(runtime.run_action("removeThenAdvance").is_err());
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(3));
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));

    runtime.run_action("observeSelection").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
    assert_eq!(runtime.value("observedBool").unwrap(), Value::Bool(true));
}

#[test]
fn persistent_scratch_designation_can_encode_the_intent_but_is_application_state() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime
        .run_action("advanceAndRemoveWithPersistentScratch")
        .unwrap();

    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );
    assert_eq!(runtime.value("observedBool").unwrap(), Value::Bool(true));
}

#[test]
fn relative_designation_is_not_currently_a_direct_structural_remove_selector() {
    let desired = r#"
state model Task { state title = "" }
state model Workstation { state tasks: [live Task] = [] }
state a: Task
state b: Task
state workstation: Workstation
state selectedTask: live Task = live a

action setup {
    workstation.tasks = [live a, live b]
    selectedTask = workstation.tasks[0]
}

action desired {
    selectedTask = next selectedTask in workstation.tasks
    remove previous selectedTask in workstation.tasks from workstation.tasks
}
"#;

    assert!(check_source_with_runtime_models(desired).is_err());
}
