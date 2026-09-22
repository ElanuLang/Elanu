use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
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

state a: Task
state b: Task
state c: Task
state workstation: Workstation
state moving: live Task = live c
state anchor: live Task = live b
state maybeAnchor: maybe live Task = none
state observed0 = ""
state observed1 = ""
state observed2 = ""
state marker = false

action setup {
    a.title = "A"
    b.title = "B"
    c.title = "C"
    workstation.tasks = [live a, live b, live c]
}

action observe {
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
}

action duplicateAnchorAndFail {
    workstation.tasks = [live a, live b, live c, live b]
    marker = true
    move moving before anchor in workstation.tasks
}

action absentAnchorAndFail {
    marker = true
    move moving before maybeAnchor in workstation.tasks
}

action moveRelativeToSelf {
    moving = workstation.tasks[1]
    anchor = workstation.tasks[1]
    move moving before anchor in workstation.tasks
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
}

action stagedViewChangeControlsMove {
    b.active = false
    anchor = workstation.tasks[0]
    move moving after anchor in workstation.visibleTasks
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
}
"#;

#[test]
fn anchor_absence_or_ambiguity_fails_transactionally() {
    let mut duplicate = runtime(SOURCE);
    duplicate.run_action("setup").unwrap();
    assert!(duplicate.run_action("duplicateAnchorAndFail").is_err());
    assert_eq!(duplicate.value("marker").unwrap(), Value::Bool(false));
    duplicate.run_action("observe").unwrap();
    assert_eq!(
        duplicate.value("observed0").unwrap(),
        Value::String("A".into())
    );
    assert_eq!(
        duplicate.value("observed1").unwrap(),
        Value::String("B".into())
    );
    assert_eq!(
        duplicate.value("observed2").unwrap(),
        Value::String("C".into())
    );

    let mut absent = runtime(SOURCE);
    absent.run_action("setup").unwrap();
    assert!(absent.run_action("absentAnchorAndFail").is_err());
    assert_eq!(absent.value("marker").unwrap(), Value::Bool(false));
}

#[test]
fn moving_relative_to_the_same_unique_child_is_a_noop() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("moveRelativeToSelf").unwrap();

    assert_eq!(
        runtime.value("observed0").unwrap(),
        Value::String("A".into())
    );
    assert_eq!(
        runtime.value("observed1").unwrap(),
        Value::String("B".into())
    );
    assert_eq!(
        runtime.value("observed2").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn staged_filter_predicate_change_controls_current_move_view() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("stagedViewChangeControlsMove").unwrap();

    // B is hidden before the move, so the current view is [A, C]. Moving C
    // after A maps that visible adjacency back to backing membership as [A, C, B].
    assert_eq!(
        runtime.value("observed0").unwrap(),
        Value::String("A".into())
    );
    assert_eq!(
        runtime.value("observed1").unwrap(),
        Value::String("C".into())
    );
    assert_eq!(
        runtime.value("observed2").unwrap(),
        Value::String("B".into())
    );
}
