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

state model Other {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
    derived visibleTasks = filter tasks as task {
        task.active
    }
}

state workstation: Workstation
state fallback: Task
state selected: maybe live Task = none
state required: live Task = live fallback
state observed = ""
state marker = 0

action setup {
    create Task in workstation as a {
        through a.title = "A"
        insert a into workstation.tasks
    }
    create Task in workstation as b {
        through b.title = "B"
        insert b into workstation.tasks
    }
    create Task in workstation as c {
        through c.title = "C"
        insert c into workstation.tasks
    }
    selected = workstation.visibleTasks[1]
    required = workstation.visibleTasks[1]
}

action nextStored {
    selected = next selected in workstation.tasks
    observed = selected.title
}

action previousFilteredRequired {
    required = previous required in workstation.visibleTasks
    observed = required.title
}

action stagedAnchorThenNext {
    selected = workstation.visibleTasks[0]
    selected = next selected in workstation.visibleTasks
    observed = selected.title
}

action stagedPredicateThenNext {
    through workstation.tasks[0].active = false
    selected = next selected in workstation.visibleTasks
    observed = selected.title
}

action stagedStructureThenNext {
    selected = workstation.tasks[2]
    create Task in workstation as d {
        through d.title = "D"
        insert d into workstation.tasks
    }
    selected = next selected in workstation.tasks
    observed = selected.title
}

action absentAnchorFails {
    marker = 1
    selected = none
    selected = next selected in workstation.tasks
}

action zeroOccurrenceFails {
    marker = 1
    remove selected from workstation.tasks
    selected = next selected in workstation.tasks
}

action setupDuplicateOccurrence {
    create Task in workstation as a {
        through a.title = "A"
        insert a into workstation.tasks
    }
    create Task in workstation as b {
        through b.title = "B"
        insert b into workstation.tasks
        insert b into workstation.tasks
    }
    create Task in workstation as c {
        through c.title = "C"
        insert c into workstation.tasks
    }
    selected = workstation.tasks[1]
}

action duplicateOccurrenceFails {
    marker = 1
    selected = next selected in workstation.tasks
}

action boundaryFails {
    marker = 1
    selected = workstation.tasks[2]
    selected = next selected in workstation.tasks
}

action laterFailureRollsBack {
    marker = 1
    selected = next selected in workstation.tasks
    fail "later failure"
}

action observeSelected {
    observed = selected.title
}
"#;

#[test]
fn next_and_previous_execute_over_stored_and_filtered_structure() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("nextStored").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );

    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("previousFilteredRequired").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("A".into())
    );
}

#[test]
fn navigation_observes_staged_anchor_predicate_and_structure_changes() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("stagedAnchorThenNext").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );

    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("stagedPredicateThenNext").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("C".into())
    );

    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    runtime.run_action("stagedStructureThenNext").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("D".into())
    );
}

#[test]
fn navigation_failures_roll_back_prior_staged_writes() {
    for action in ["absentAnchorFails", "zeroOccurrenceFails", "boundaryFails"] {
        let mut runtime = make_runtime(SOURCE);
        runtime.run_action("setup").unwrap();
        assert!(runtime.run_action(action).is_err(), "{action} should fail");
        assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
        runtime.run_action("observeSelected").unwrap();
        assert_eq!(
            runtime.value("observed").unwrap(),
            Value::String("B".into())
        );
    }

    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setupDuplicateOccurrence").unwrap();
    assert!(runtime.run_action("duplicateOccurrenceFails").is_err());
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    runtime.run_action("observeSelected").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn later_failure_rolls_back_successful_navigation() {
    let mut runtime = make_runtime(SOURCE);
    runtime.run_action("setup").unwrap();
    assert!(runtime.run_action("laterFailureRollsBack").is_err());
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    runtime.run_action("observeSelected").unwrap();
    assert_eq!(
        runtime.value("observed").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn relative_navigation_rejects_ordinary_value_context() {
    let source = SOURCE.replace(
        "state observed = \"\"",
        "state observed = \"\"\nderived invalid = next selected in workstation.tasks",
    );
    let errors =
        check_source_with_runtime_models(&source).expect_err("ordinary value use should fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("only valid in a compatible live designation context")));
}

#[test]
fn relative_navigation_rejects_anchor_source_model_mismatch() {
    let source = r#"
state model Task { state title = "" }
state model Other { state title = "" }
state model Holder { state others: [live Other] = [] }
state task: Task
state holder: Holder
state selected: live Task = live task

action invalid {
    selected = next selected in holder.others
}
"#;
    let errors = check_source_with_runtime_models(source).expect_err("model mismatch should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("relative-navigation anchor")
            && error.message.contains("source contains live Other")));
}
