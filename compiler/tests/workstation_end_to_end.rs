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
    state points = 1
}

state model Workstation {
    state search = ""
    state selected = 0
    state tasks: [live Task] = []

    derived visibleTasks = filter tasks as task {
        task.active and (search == "" or task.title contains search ignoring case)
    }

    derived visiblePoints = reduce visibleTasks from 0 as (sum, task) {
        sum + task.points
    }
}

state workstation: Workstation
state observedPoints = 0
state observedSelectedPoints = 0
state observedSelectedTitle = ""
derived observedSearch = workstation.search

action setPoints(state target: Int, value: Int) {
    target = value
}

action seed {
    create Task in workstation as first {
        through first.title = "Steel Bolt"
        through first.points = 3
        insert first into workstation.tasks
    }
    create Task in workstation as second {
        through second.title = "Brass Washer"
        through second.points = 5
        insert second into workstation.tasks
    }
    create Task in workstation as third {
        through third.title = "Bolt Cutter"
        through third.active = false
        through third.points = 8
        insert third into workstation.tasks
    }

    observedPoints = workstation.visiblePoints
}

action searchBoltAndRead {
    workstation.search = "BOLT"
    observedPoints = workstation.visiblePoints
    observedSelectedPoints = workstation.visibleTasks[workstation.selected].points
    observedSelectedTitle = workstation.visibleTasks[workstation.selected].title
}

action mutateSelectedDirectly {
    workstation.search = "bolt"
    through workstation.visibleTasks[workstation.selected].points += 2
    observedPoints = workstation.visiblePoints
    observedSelectedPoints = workstation.visibleTasks[workstation.selected].points
}

action mutateSelectedThroughAuthority {
    workstation.search = "BOLT"
    setPoints(state through workstation.visibleTasks[workstation.selected].points, 9)
    observedPoints = workstation.visiblePoints
    observedSelectedPoints = workstation.visibleTasks[workstation.selected].points
}

action activateThirdAndObserve {
    workstation.search = "bolt"
    through workstation.tasks[2].active = true
    observedPoints = workstation.visiblePoints
    workstation.selected = 1
    observedSelectedTitle = workstation.visibleTasks[workstation.selected].title
}

action changeSearchAndFail {
    workstation.search = "washer"
    observedPoints = workstation.visiblePoints
    fail "rollback workstation transition"
}
"#;

#[test]
fn workstation_composes_create_insert_filter_search_reduce_and_indexed_read() {
    let mut runtime = runtime(SOURCE);

    runtime.run_action("seed").expect("seed should commit");
    assert_eq!(runtime.value("observedPoints").unwrap(), Value::Int(8));

    runtime
        .run_action("searchBoltAndRead")
        .expect("search/read should commit");
    assert_eq!(runtime.value("observedPoints").unwrap(), Value::Int(3));
    assert_eq!(
        runtime.value("observedSelectedPoints").unwrap(),
        Value::Int(3)
    );
    assert_eq!(
        runtime.value("observedSelectedTitle").unwrap(),
        Value::String("Steel Bolt".to_string())
    );
}

#[test]
fn workstation_filtered_selection_composes_with_direct_and_forwarded_mutation() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").unwrap();

    runtime.run_action("mutateSelectedDirectly").unwrap();
    assert_eq!(runtime.value("observedPoints").unwrap(), Value::Int(5));
    assert_eq!(
        runtime.value("observedSelectedPoints").unwrap(),
        Value::Int(5)
    );

    runtime
        .run_action("mutateSelectedThroughAuthority")
        .unwrap();
    assert_eq!(runtime.value("observedPoints").unwrap(), Value::Int(9));
    assert_eq!(
        runtime.value("observedSelectedPoints").unwrap(),
        Value::Int(9)
    );
}

#[test]
fn workstation_staged_business_state_changes_recompute_current_search_view() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").unwrap();

    runtime.run_action("activateThirdAndObserve").unwrap();
    assert_eq!(runtime.value("observedPoints").unwrap(), Value::Int(11));
    assert_eq!(
        runtime.value("observedSelectedTitle").unwrap(),
        Value::String("Bolt Cutter".to_string())
    );
}

#[test]
fn workstation_failure_rolls_back_search_and_derived_observation_state() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").unwrap();
    runtime.run_action("searchBoltAndRead").unwrap();

    let error = runtime
        .run_action("changeSearchAndFail")
        .expect_err("transition should fail");
    assert!(error.message.contains("rollback workstation transition"));

    assert_eq!(
        runtime.value("observedSearch").unwrap(),
        Value::String("BOLT".to_string())
    );
    assert_eq!(runtime.value("observedPoints").unwrap(), Value::Int(3));
}
