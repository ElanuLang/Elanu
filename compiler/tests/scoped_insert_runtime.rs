use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 5.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0.0 as (sum, line) {
        sum + line.lineTotal
    }
}

state invoice: Invoice
state observed = 0.0

action addLine {
    create LineItem in invoice as line {
        through line.quantity = 3
        insert line into invoice.lines
        observed = invoice.total
    }
}

action addTwo {
    create LineItem in invoice as first {
        through first.quantity = 2
        insert first into invoice.lines
    }
    create LineItem in invoice as second {
        through second.quantity = 4
        insert second into invoice.lines
    }
    observed = invoice.total
}

action addThenFail {
    create LineItem in invoice as line {
        through line.quantity = 9
        insert line into invoice.lines
        observed = invoice.total
        fail "rollback insertion"
    }
}
"#;

#[test]
fn scoped_insert_adds_the_exact_fresh_designation_to_membership() {
    let mut runtime = runtime(SOURCE);

    runtime.run_action("addLine").expect("insert should commit");

    assert_eq!(runtime.value("observed").unwrap(), Value::Float(15.0));
    let Value::Sequence {
        element_model,
        targets,
    } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should be realized as a runtime sequence");
    };
    assert_eq!(element_model, "LineItem");
    assert_eq!(targets.len(), 1);
}

#[test]
fn repeated_scoped_insertions_preserve_distinct_identity_and_order() {
    let mut runtime = runtime(SOURCE);

    runtime
        .run_action("addTwo")
        .expect("both inserts should commit");

    assert_eq!(runtime.value("observed").unwrap(), Value::Float(30.0));
    let Value::Sequence {
        element_model,
        targets,
    } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should be realized as a runtime sequence");
    };
    assert_eq!(element_model, "LineItem");
    assert_eq!(targets.len(), 2);
    assert_ne!(targets[0], targets[1]);
}

#[test]
fn failed_scoped_insert_rolls_back_membership_and_fresh_identity() {
    let mut runtime = runtime(SOURCE);

    let error = runtime
        .run_action("addThenFail")
        .expect_err("failure should roll back insertion");

    assert!(error.message.contains("rollback insertion"));
    assert_eq!(runtime.value("observed").unwrap(), Value::Float(0.0));
    assert_eq!(
        runtime.value("__elanu_mseq$invoice$lines").unwrap(),
        Value::Sequence {
            element_model: "LineItem".to_string(),
            targets: Vec::new(),
        }
    );
}

#[test]
fn fresh_scoped_insert_can_enter_compatible_cross_owner_membership() {
    let mut runtime = runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice
state other: Invoice
state selected: maybe live LineItem = none

action seed {
    create LineItem in invoice as line {
        insert line into other.lines
    }
    selected = other.lines[0]
}

action attachToOwner {
    insert selected into invoice.lines
}
"#,
    );

    runtime
        .run_action("seed")
        .expect("cross-owner membership should commit");
    runtime
        .run_action("attachToOwner")
        .expect("child should remain rooted in its creation owner");

    let Value::Sequence {
        targets: owner_lines,
        ..
    } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should be a runtime sequence");
    };
    let Value::Sequence {
        targets: other_lines,
        ..
    } = runtime.value("__elanu_mseq$other$lines").unwrap()
    else {
        panic!("other.lines should be a runtime sequence");
    };
    assert_eq!(owner_lines.len(), 1);
    assert_eq!(other_lines.len(), 1);
    assert_eq!(owner_lines[0], other_lines[0]);
}

#[test]
fn insertion_itself_forces_runtime_sequence_realization() {
    let mut runtime = runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice

action add {
    create LineItem in invoice as line {
        insert line into invoice.lines
    }
}
"#,
    );

    assert_eq!(
        runtime.value("__elanu_mseq$invoice$lines").unwrap(),
        Value::Sequence {
            element_model: "LineItem".to_string(),
            targets: Vec::new(),
        }
    );

    runtime.run_action("add").expect("insert should commit");

    let Value::Sequence {
        element_model,
        targets,
    } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should remain a runtime sequence");
    };
    assert_eq!(element_model, "LineItem");
    assert_eq!(targets.len(), 1);
}

#[test]
fn existing_designation_can_move_between_same_owner_memberships() {
    let mut runtime = runtime(
        r#"
state model Task {
    state done = false
}

state model Board {
    state backlog: [live Task] = []
    state completed: [live Task] = []
}

state board: Board
state selected: maybe live Task = none

action seed {
    create Task in board as task {
        insert task into board.backlog
    }
    selected = board.backlog[0]
}

action complete {
    remove selected from board.backlog
    insert selected into board.completed
}
"#,
    );

    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("complete")
        .expect("same-owner transfer should commit");

    let Value::Sequence {
        targets: backlog, ..
    } = runtime.value("__elanu_mseq$board$backlog").unwrap()
    else {
        panic!("board.backlog should be a runtime sequence");
    };
    let Value::Sequence {
        targets: completed, ..
    } = runtime.value("__elanu_mseq$board$completed").unwrap()
    else {
        panic!("board.completed should be a runtime sequence");
    };
    assert!(backlog.is_empty());
    assert_eq!(completed.len(), 1);
}

#[test]
fn existing_designation_can_enter_cross_owner_membership_without_reparenting() {
    let mut runtime = runtime(
        r#"
state model Task {
    state done = false
}

state model Board {
    state tasks: [live Task] = []
}

state left: Board
state right: Board
state selected: maybe live Task = none

action seed {
    create Task in left as task {
        insert task into left.tasks
    }
    selected = left.tasks[0]
}

action share {
    insert selected into right.tasks
}
"#,
    );

    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("share")
        .expect("cross-owner membership should not reparent the child");

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
}
