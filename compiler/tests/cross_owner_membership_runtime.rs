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

state model Board {
    state tasks: [live Task] = []

    derived activeTasks = filter tasks as task {
        task.active
    }

    derived activeCount = reduce activeTasks from 0 as (count, task) {
        count + 1
    }
}

state left: Board
state right: Board
state selected: maybe live Task = none
state observedTitle = ""
state observedCount = 0
state selectedStillVisible = false

action rename(state target: String, value: String) {
    target = value
}

action seedCrossOwnerMembership {
    create Task in left as task {
        through task.title = "Shared"
        insert task into right.tasks
    }
    selected = right.tasks[0]
    observedTitle = selected.title
    observedCount = right.activeCount
    selectedStillVisible = selected is in right.activeTasks
}

action attachToRootOwnerAndMutateThroughForeignMembership {
    insert selected into left.tasks
    through right.tasks[0].title = "Edited"
    observedTitle = left.tasks[0].title
}

action grantThroughForeignMembership {
    rename(state through right.tasks[0].title, "Granted")
    observedTitle = left.tasks[0].title
}

action changeForeignViewMembership {
    through right.tasks[0].active = false
    observedCount = right.activeCount
    selectedStillVisible = selected is in right.activeTasks
}

action failAfterForeignStructuralAndChildWrites {
    through right.tasks[0].title = "Temporary"
    remove selected from right.tasks
    fail "rollback cross-owner work"
}
"#;

#[test]
fn cross_owner_membership_preserves_identity_reads_authority_views_and_rollback() {
    let mut runtime = runtime(SOURCE);

    runtime
        .run_action("seedCrossOwnerMembership")
        .expect("fresh child should enter non-owning cross-owner membership");
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Shared".into())
    );
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(1));
    assert_eq!(
        runtime.value("selectedStillVisible").unwrap(),
        Value::Bool(true)
    );

    runtime
        .run_action("attachToRootOwnerAndMutateThroughForeignMembership")
        .expect("same identity should remain rooted in left while usable through right");
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Edited".into())
    );

    let Value::Sequence {
        targets: left_tasks,
        ..
    } = runtime.value("__elanu_mseq$left$tasks").unwrap()
    else {
        panic!("left.tasks should be a runtime sequence");
    };
    let Value::Sequence {
        targets: right_tasks,
        ..
    } = runtime.value("__elanu_mseq$right$tasks").unwrap()
    else {
        panic!("right.tasks should be a runtime sequence");
    };
    assert_eq!(left_tasks.len(), 1);
    assert_eq!(right_tasks.len(), 1);
    assert_eq!(left_tasks[0], right_tasks[0]);

    runtime
        .run_action("grantThroughForeignMembership")
        .expect("state-through should resolve the same cross-owner child state");
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Granted".into())
    );

    let error = runtime
        .run_action("failAfterForeignStructuralAndChildWrites")
        .expect_err("later failure should roll back cross-owner structural and child writes");
    assert!(error.message.contains("rollback cross-owner work"));

    let Value::Sequence {
        targets: right_after_rollback,
        ..
    } = runtime.value("__elanu_mseq$right$tasks").unwrap()
    else {
        panic!("right.tasks should remain a runtime sequence");
    };
    assert_eq!(right_after_rollback.len(), 1);
    assert_eq!(right_after_rollback[0], left_tasks[0]);
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Granted".into())
    );

    runtime
        .run_action("changeForeignViewMembership")
        .expect("foreign membership should participate in filter and reduction updates");
    assert_eq!(runtime.value("observedCount").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("selectedStillVisible").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn filtered_move_in_foreign_membership_changes_only_that_membership_order() {
    let mut runtime = runtime(
        r#"
state model Task {
    state active = true
}

state model Board {
    state tasks: [live Task] = []

    derived activeTasks = filter tasks as task {
        task.active
    }
}

state left: Board
state right: Board
state moving: maybe live Task = none
state anchor: maybe live Task = none

action seed {
    create Task in left as first {
        insert first into left.tasks
        insert first into right.tasks
    }
    create Task in left as second {
        insert second into left.tasks
        insert second into right.tasks
    }
    moving = right.tasks[1]
    anchor = right.tasks[0]
}

action reorderForeignView {
    move moving before anchor in right.activeTasks
}
"#,
    );

    runtime.run_action("seed").expect("seed should commit");

    let Value::Sequence {
        targets: left_before,
        ..
    } = runtime.value("__elanu_mseq$left$tasks").unwrap()
    else {
        panic!("left.tasks should be a runtime sequence");
    };
    let Value::Sequence {
        targets: right_before,
        ..
    } = runtime.value("__elanu_mseq$right$tasks").unwrap()
    else {
        panic!("right.tasks should be a runtime sequence");
    };

    assert_eq!(left_before, right_before);
    assert_eq!(left_before.len(), 2);

    runtime
        .run_action("reorderForeignView")
        .expect("foreign filtered membership should support ordinary structural movement");

    let Value::Sequence {
        targets: left_after,
        ..
    } = runtime.value("__elanu_mseq$left$tasks").unwrap()
    else {
        panic!("left.tasks should remain a runtime sequence");
    };
    let Value::Sequence {
        targets: right_after,
        ..
    } = runtime.value("__elanu_mseq$right$tasks").unwrap()
    else {
        panic!("right.tasks should remain a runtime sequence");
    };

    assert_eq!(
        left_after, left_before,
        "foreign movement must not mutate the rooting owner's membership"
    );
    assert_eq!(
        right_after,
        vec![left_before[1].clone(), left_before[0].clone()],
        "foreign movement should reorder the same child identities only in the selected membership",
    );
}
