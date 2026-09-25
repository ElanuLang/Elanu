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

state a: Task
state hidden1: Task
state b: Task
state hidden2: Task
state d: Task
state workstation: Workstation
state moving: live Task = live d
state anchor: live Task = live b
state maybeMoving: maybe live Task = none
state observed0 = ""
state observed1 = ""
state observed2 = ""
state observed3 = ""
state observed4 = ""
state marker = false

action setup {
    a.title = "A"
    hidden1.title = "H1"
    b.title = "B"
    hidden2.title = "H2"
    d.title = "D"
    hidden1.active = false
    hidden2.active = false
    workstation.tasks = [live a, live hidden1, live b, live hidden2, live d]
}

action observeBacking {
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
    observed3 = workstation.tasks[3].title
    observed4 = workstation.tasks[4].title
}

action moveDirect {
    move moving after anchor in workstation.tasks
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
    observed3 = workstation.tasks[3].title
    observed4 = workstation.tasks[4].title
}

action moveThroughView {
    move moving before anchor in workstation.visibleTasks
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
    observed3 = workstation.tasks[3].title
    observed4 = workstation.tasks[4].title
}

action moveScopedThroughView {
    with workstation.tasks[4] as localMoving {
        move localMoving before anchor in workstation.visibleTasks
    }
    observed0 = workstation.tasks[0].title
    observed1 = workstation.tasks[1].title
    observed2 = workstation.tasks[2].title
    observed3 = workstation.tasks[3].title
    observed4 = workstation.tasks[4].title
}

action stagedReselectAndMove {
    moving = workstation.tasks[0]
    anchor = workstation.tasks[4]
    move moving after anchor in workstation.tasks
    observed0 = workstation.tasks[0].title
    observed4 = workstation.tasks[4].title
}

action duplicateMovingAndFail {
    workstation.tasks = [live a, live d, live d, live b]
    marker = true
    move moving before anchor in workstation.tasks
}

action absentMovingAndFail {
    marker = true
    move maybeMoving before anchor in workstation.tasks
}

action moveThenFail {
    marker = true
    move moving before anchor in workstation.visibleTasks
    fail "rollback movement"
}
"#;

fn observed(runtime: &mut Runtime) -> Vec<String> {
    (0..5)
        .map(
            |index| match runtime.value(&format!("observed{index}")).unwrap() {
                Value::String(value) => value,
                other => panic!("expected String observation, found {other:?}"),
            },
        )
        .collect()
}

#[test]
fn stored_membership_move_uses_child_identity_and_updates_current_order() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("moveDirect").unwrap();

    assert_eq!(observed(&mut runtime), ["A", "H1", "B", "D", "H2"]);
}

#[test]
fn filtered_view_move_maps_to_backing_and_preserves_hidden_relative_order() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("moveThroughView").unwrap();

    assert_eq!(observed(&mut runtime), ["A", "H1", "D", "B", "H2"]);
}

#[test]
fn scoped_designations_can_drive_structural_move_without_becoming_authority() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("moveScopedThroughView").unwrap();

    assert_eq!(observed(&mut runtime), ["A", "H1", "D", "B", "H2"]);
}

#[test]
fn movement_uses_transaction_visible_designation_reselection() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("stagedReselectAndMove").unwrap();

    assert_eq!(
        runtime.value("observed0").unwrap(),
        Value::String("H1".into())
    );
    assert_eq!(
        runtime.value("observed4").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn ambiguous_or_absent_moving_designation_fails_and_rolls_back() {
    let mut duplicate = make_runtime(SOURCE);
    duplicate.run_action("setup").unwrap();
    assert!(duplicate.run_action("duplicateMovingAndFail").is_err());
    assert_eq!(duplicate.value("marker").unwrap(), Value::Bool(false));
    duplicate.run_action("observeBacking").unwrap();
    assert_eq!(observed(&mut duplicate), ["A", "H1", "B", "H2", "D"]);

    let mut absent = make_runtime(SOURCE);
    absent.run_action("setup").unwrap();
    assert!(absent.run_action("absentMovingAndFail").is_err());
    assert_eq!(absent.value("marker").unwrap(), Value::Bool(false));
}

#[test]
fn later_failure_rolls_back_structural_move_with_the_surrounding_action() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert!(runtime.run_action("moveThenFail").is_err());
    assert_eq!(runtime.value("marker").unwrap(), Value::Bool(false));

    runtime.run_action("observeBacking").unwrap();
    assert_eq!(observed(&mut runtime), ["A", "H1", "B", "H2", "D"]);
}

#[test]
fn structural_move_rejects_wrong_model_without_private_name_leakage() {
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
state moving: live Other = live other
state anchor: live Task = live task

action bad {
    workstation.tasks = [live task]
    move moving before anchor in workstation.tasks
}
"#;

    let errors = check_source_with_runtime_models(source).expect_err("wrong model should fail");
    let rendered = errors
        .iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("moving live designation 'moving' has type live Other"));
    assert!(!rendered.contains("__elanu"));
}
