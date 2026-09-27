use elanu_compiler::{
    check_source, check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

#[test]
fn statically_named_children_can_reorder_by_whole_sequence_replacement() {
    let checked = check_source(
        r#"
state model Task {
    state title = ""
}

state a: Task
state b: Task
state c: Task
state order: [live Task] = [live a, live b, live c]
derived firstTitle = order[0].title

action seed {
    a.title = "A"
    b.title = "B"
    c.title = "C"
}

action reorder {
    order = [live c, live a, live b]
}
"#,
    )
    .expect("static whole-sequence replacement is the control case");

    let mut runtime = Runtime::from_program(&checked).expect("runtime should initialize");
    runtime.run_action("seed").unwrap();
    runtime.run_action("reorder").unwrap();
    assert_eq!(
        runtime.value("firstTitle").unwrap(),
        Value::String("C".into())
    );
}

#[test]
fn runtime_created_membership_cannot_be_rebuilt_from_one_captured_child_without_reconstruction() {
    let source = r#"
state model Task {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state workstation: Workstation
state selectedTask: maybe live Task = none

action seed {
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
    selectedTask = workstation.tasks[1]
}

action tryWholeReplacement {
    with selectedTask as moving {
        workstation.tasks = [live moving]
    }
}
"#;

    let diagnostics = check_source_with_runtime_models(source)
        .expect_err("capturing one child must not magically expose the rest of runtime membership for whole-value rebuild");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!messages.contains("__elanu_"));
}

#[test]
fn existing_designation_cannot_currently_detach_and_reinsert_as_a_general_move_primitive() {
    let source = r#"
state model Task {
    state title = ""
}

state model Workstation {
    state tasks: [live Task] = []
}

state workstation: Workstation
state selectedTask: maybe live Task = none

action seed {
    create Task in workstation as alpha {
        insert alpha into workstation.tasks
    }
    create Task in workstation as bravo {
        insert bravo into workstation.tasks
    }
    selectedTask = workstation.tasks[0]
}

action tryDetachReinsert {
    with selectedTask as moving {
        remove moving from workstation.tasks
        insert moving into workstation.tasks
    }
}
"#;

    let diagnostics = check_source_with_runtime_models(source)
        .expect_err("fresh-create insertion must not already imply arbitrary reinsertion of an existing designation");
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!messages.contains("__elanu_"));
}
