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

    derived taskCount = reduce tasks from 0 as (count, task) {
        count + 1
    }
}

state workstation: Workstation
state selectedTask: maybe live Task = none
state observedTitle = ""
state movedSelectionTitle = ""
state observedCount = 0
state selectedStillVisible = false

derived taskCount = workstation.taskCount

action rename(state target: String, value: String) {
    target = value
}

action seedWorkspace {
    create Task in workstation as alpha {
        through alpha.title = "Alpha"
        insert alpha into workstation.tasks
    }
    create Task in workstation as bravo {
        through bravo.title = "Bravo"
        insert bravo into workstation.tasks
    }
    create Task in workstation as charlie {
        through charlie.title = "Charlie"
        insert charlie into workstation.tasks
    }
    create Task in workstation as delta {
        through delta.title = "Delta"
        insert delta into workstation.tasks
    }
    selectedTask = workstation.visibleTasks[1]
    movedSelectionTitle = ""
}

action hideEarlierAndAdvance {
    through workstation.tasks[0].active = false
    selectedStillVisible = selectedTask is in workstation.visibleTasks
    selectedTask = next selectedTask in workstation.visibleTasks
    observedTitle = selectedTask.title
}

action renameSelectedThroughAuthority {
    with selectedTask as captured {
        rename(state through captured.title, "Charlie edited")
        observedTitle = captured.title
    }
}

action removeSelectedAndContinue {
    with selectedTask as removalTarget {
        selectedTask = next removalTarget in workstation.visibleTasks
        remove removalTarget from workstation.visibleTasks
        observedTitle = selectedTask.title
        observedCount = workstation.taskCount
    }
}

action moveMutateRemoveAndContinue {
    with previous selectedTask in workstation.visibleTasks as anchor {
        move selectedTask before anchor in workstation.visibleTasks
    }
    with selectedTask as moving {
        rename(state through moving.title, "Charlie edited")
        movedSelectionTitle = selectedTask.title
        selectedTask = next moving in workstation.visibleTasks
        remove moving from workstation.visibleTasks
        observedTitle = selectedTask.title
        observedCount = workstation.taskCount
    }
}

action narrowFilterThenContinue {
    workstation.search = "delta"
    selectedStillVisible = selectedTask is in workstation.visibleTasks
    observedTitle = selectedTask.title
}
"#;

#[test]
fn realistic_workstation_flow_composes_without_key_position_or_scratch_state() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("seedWorkspace").unwrap();
    assert_eq!(runtime.value("taskCount").unwrap(), Value::Int(4));

    runtime.run_action("hideEarlierAndAdvance").unwrap();
    assert_eq!(
        runtime.value("selectedStillVisible").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Charlie".into())
    );

    runtime
        .run_action("renameSelectedThroughAuthority")
        .unwrap();
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Charlie edited".into())
    );

    runtime.run_action("removeSelectedAndContinue").unwrap();
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(3));
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Delta".into())
    );

    runtime.run_action("narrowFilterThenContinue").unwrap();
    assert_eq!(
        runtime.value("selectedStillVisible").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Delta".into())
    );
}

#[test]
fn full_v09_workstation_flow_composes_through_move_mutation_navigation_and_removal() {
    let mut runtime = make_runtime(SOURCE);

    runtime.run_action("seedWorkspace").unwrap();
    runtime.run_action("hideEarlierAndAdvance").unwrap();
    runtime.run_action("moveMutateRemoveAndContinue").unwrap();

    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(3));
    assert_eq!(
        runtime.value("movedSelectionTitle").unwrap(),
        Value::String("Charlie edited".into())
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Bravo".into())
    );

    runtime.run_action("narrowFilterThenContinue").unwrap();
    assert_eq!(
        runtime.value("selectedStillVisible").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Bravo".into())
    );
}
