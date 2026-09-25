use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

// Source-contract composition test: a live modeled child identity may itself be the
// explicit rooting owner of another dynamic child. Designation remains non-owning; the
// owner operand carries identity only in this explicit rooting-provenance position,
// not as a newly general ordinary designation value.
const SOURCE: &str = r#"
state model Project {
    state name = ""
}

state model Task {
    state title = ""
}

state model Workspace {
    state projects: [live Project] = []
    state tasks: [live Task] = []
}

state workspace: Workspace
state selectedProject: maybe live Project = none
state selectedTask: maybe live Task = none
state otherProject: maybe live Project = none

action seed {
    create Project in workspace as project {
        through project.name = "Primary"
        insert project into workspace.projects

        create Task in project as task {
            through task.title = "Nested"
            insert task into workspace.tasks
        }
    }

    create Project in workspace as project {
        through project.name = "Other"
        insert project into workspace.projects
    }

    selectedProject = workspace.projects[0]
    otherProject = workspace.projects[1]
    selectedTask = workspace.tasks[0]
}

action addTaskLater {
    create Task in selectedProject as task {
        through task.title = "Later"
        insert task into workspace.tasks
    }
}

action destroyParentTooEarly {
    destroy selectedProject in workspace
}

action destroyTaskThroughWrongProject {
    destroy selectedTask in otherProject
}

action destroySelectedTask {
    destroy selectedTask in selectedProject
}

action destroySelectedProject {
    destroy selectedProject in workspace
}
"#;

fn runtime() -> Runtime {
    let checked =
        check_source_with_runtime_models(SOURCE).expect("dynamic-owner source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

fn targets(runtime: &mut Runtime, name: &str) -> Vec<String> {
    let Value::Sequence { targets, .. } = runtime.value(name).expect("sequence should exist")
    else {
        panic!("{name} should be a runtime sequence");
    };
    targets
}

#[test]
fn dynamic_child_can_root_nested_child_without_becoming_owning_designation() {
    let mut runtime = runtime();
    runtime.run_action("seed").expect("seed should commit");

    let projects = targets(&mut runtime, "__elanu_mseq$workspace$projects");
    let tasks = targets(&mut runtime, "__elanu_mseq$workspace$tasks");
    assert_eq!(projects.len(), 2);
    assert_eq!(tasks.len(), 1);
    let primary_project = projects[0].clone();
    let nested_task = tasks[0].clone();

    let error = runtime
        .run_action("destroyParentTooEarly")
        .expect_err("a live rooted descendant must still block parent destruction");
    assert!(error.message.contains("roots another live child"));
    assert_eq!(
        runtime.value("__elanu_live$selectedProject").unwrap(),
        Value::String(primary_project.clone())
    );

    let error = runtime
        .run_action("destroyTaskThroughWrongProject")
        .expect_err("another project designation must not gain lifetime authority");
    assert!(error.message.contains("rooting owner"));
    assert_eq!(
        runtime.value("__elanu_live$selectedTask").unwrap(),
        Value::String(nested_task)
    );

    // A failed foreign-owner claim must not disturb the real provenance relation:
    // the task is still rooted in the selected project, so its parent still cannot end.
    let error = runtime
        .run_action("destroyParentTooEarly")
        .expect_err("failed foreign-owner proof must preserve the rooted descendant");
    assert!(error.message.contains("roots another live child"));

    runtime
        .run_action("destroySelectedTask")
        .expect("the recorded dynamic owner should authorize child destruction");
    assert!(targets(&mut runtime, "__elanu_mseq$workspace$tasks").is_empty());
    assert_eq!(
        runtime.value("__elanu_live$selectedTask").unwrap(),
        Value::String(String::new())
    );

    runtime
        .run_action("destroySelectedProject")
        .expect("parent should become destroyable after its descendant ends");
    assert_eq!(
        targets(&mut runtime, "__elanu_mseq$workspace$projects").len(),
        1
    );
    assert_eq!(
        runtime.value("__elanu_live$selectedProject").unwrap(),
        Value::String(String::new())
    );
}

#[test]
fn committed_dynamic_owner_can_root_additional_children_later() {
    let mut runtime = runtime();
    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("addTaskLater")
        .expect("persistent project designation should carry exact owner identity");

    assert_eq!(
        targets(&mut runtime, "__elanu_mseq$workspace$tasks").len(),
        2
    );
}

#[test]
fn absent_optional_owner_fails_nested_creation_transactionally() {
    let source = r#"
state model Project { state name = "" }
state model Task { state title = "" }
state model Workspace { state tasks: [live Task] = [] }
state workspace: Workspace
state selectedProject: maybe live Project = none

action attempt {
    create Task in selectedProject as task {
        insert task into workspace.tasks
    }
}
"#;

    let checked = check_source_with_runtime_models(source).expect("source shape should check");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    let error = runtime
        .run_action("attempt")
        .expect_err("absent maybe-live owner must not invent an owner identity");
    assert!(error.message.contains("owner"));
    assert!(targets(&mut runtime, "__elanu_mseq$workspace$tasks").is_empty());
}
