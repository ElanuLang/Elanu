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
    state selected = 0
    state tasks: [live Task] = []

    derived visibleTasks = filter tasks as task {
        task.active
    }

    derived visibleCount = reduce visibleTasks from 0 as (count, task) {
        count + 1
    }
}

state workstation: Workstation
state observedSelected = 99
state observedTitle = ""
state observedCount = 99

derived currentSelected = workstation.selected

action seedThree {
    create Task in workstation as first {
        through first.title = "A"
        insert first into workstation.tasks
    }
    create Task in workstation as second {
        through second.title = "B"
        insert second into workstation.tasks
    }
    create Task in workstation as third {
        through third.title = "C"
        insert third into workstation.tasks
    }
}

action seedOne {
    create Task in workstation as only {
        through only.title = "Only"
        insert only into workstation.tasks
    }
}

action selectFirst {
    workstation.selected = 0
}

action selectMiddle {
    workstation.selected = 1
}

action selectLast {
    workstation.selected = 2
}

action normalizeSelection {
    if workstation.visibleCount == 0 {
        workstation.selected = 0 - 1
    } else {
        if workstation.selected >= workstation.visibleCount {
            workstation.selected = workstation.visibleCount - 1
        }
    }
}

action observeSelection {
    observedSelected = workstation.selected
    observedCount = workstation.visibleCount
    if workstation.selected >= 0 {
        observedTitle = workstation.visibleTasks[workstation.selected].title
    }
}

action removeSelectedAndNormalize {
    remove workstation.visibleTasks[workstation.selected]
    normalizeSelection()
    observeSelection()
}

action hideSelectedAndNormalize {
    through workstation.visibleTasks[workstation.selected].active = false
    normalizeSelection()
    observeSelection()
}

action removeThenFail {
    remove workstation.visibleTasks[workstation.selected]
    normalizeSelection()
    fail "rollback selection continuity"
}
"#;

#[test]
fn removing_first_row_keeps_same_index_when_next_row_slides_into_it() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seedThree").unwrap();
    runtime.run_action("selectFirst").unwrap();
    runtime.run_action("removeSelectedAndNormalize").unwrap();

    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn removing_middle_row_keeps_same_index_when_next_row_slides_into_it() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seedThree").unwrap();
    runtime.run_action("selectMiddle").unwrap();
    runtime.run_action("removeSelectedAndNormalize").unwrap();

    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn removing_last_row_clamps_selection_to_new_last_row() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seedThree").unwrap();
    runtime.run_action("selectLast").unwrap();
    runtime.run_action("removeSelectedAndNormalize").unwrap();

    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("B".into())
    );
}

#[test]
fn removing_only_row_uses_negative_int_sentinel_and_avoids_indexing_empty_view() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seedOne").unwrap();
    runtime.run_action("selectFirst").unwrap();
    runtime.run_action("removeSelectedAndNormalize").unwrap();

    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(-1));
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("".into())
    );
}

#[test]
fn predicate_driven_disappearance_uses_same_selection_policy() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seedThree").unwrap();
    runtime.run_action("selectMiddle").unwrap();
    runtime.run_action("hideSelectedAndNormalize").unwrap();

    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(2));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn failure_rolls_back_structural_edit_and_nested_normalization_together() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seedThree").unwrap();
    runtime.run_action("selectLast").unwrap();

    assert!(runtime.run_action("removeThenFail").is_err());
    assert_eq!(runtime.value("currentSelected").unwrap(), Value::Int(2));
}
