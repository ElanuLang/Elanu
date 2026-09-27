use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

// Contract: every dynamic Project retains its own declared structural Task membership
// and derived view; persistent project selection carries identity, not copied owner data.
// Structural membership remains non-owning even when reached through that dynamic owner identity.
// Root provenance, project-local membership, persistent selection, and writable authority stay distinct.
// Indexing a dynamically reached [live T] member preserves that full owner-relative source path.
// The representation must not require an application-maintained project key or mirrored task table.
const SOURCE: &str = r#"
state model Task {
    state title = ""
    state active = true
}

state model Project {
    state tasks: [live Task] = []
    state search = ""

    derived visibleTasks = filter tasks as task {
        task.active and (search == "" or task.title contains search ignoring case)
    }
}

state model Workspace {
    state projects: [live Project] = []
}

state workspace: Workspace
state projectA: maybe live Project = none
state projectB: maybe live Project = none
state selectedProject: maybe live Project = none
state selectedTask: maybe live Task = none
state observedTitle = ""
state selectedStillVisible = false

action seed {
    create Project in workspace as project {
        projectA = project
        selectedProject = project
        insert project into workspace.projects
    }

    create Task in projectA as task {
        through task.title = "Alpha"
        insert task into projectA.tasks
        selectedTask = task
    }
    create Task in projectA as task {
        through task.title = "Bravo"
        insert task into projectA.tasks
    }

    create Project in workspace as project {
        projectB = project
        insert project into workspace.projects
    }
    create Task in projectB as task {
        through task.title = "Delta"
        insert task into projectB.tasks
    }
}

action filterAndSelect {
    through selectedProject.search = "bravo"
    selectedStillVisible = selectedTask is in selectedProject.visibleTasks
    selectedTask = selectedProject.visibleTasks[0]
    observedTitle = selectedTask.title
}

action switchProject {
    selectedProject = projectB
    selectedTask = selectedProject.tasks[0]
    observedTitle = selectedTask.title
}

action destroySelected {
    destroy selectedTask in selectedProject
}
"#;

fn runtime() -> Runtime {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dynamic project editor composition should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

#[test]
fn project_local_membership_filtering_switching_and_destroy_compose_by_identity() {
    let mut runtime = runtime();

    runtime.run_action("seed").expect("seed should commit");

    runtime
        .run_action("filterAndSelect")
        .expect("project-local filter selection should commit");
    assert_eq!(
        runtime.value("selectedStillVisible").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Bravo".into())
    );

    runtime
        .run_action("switchProject")
        .expect("switching projects should commit");
    assert_eq!(
        runtime.value("observedTitle").unwrap(),
        Value::String("Delta".into())
    );

    runtime
        .run_action("destroySelected")
        .expect("destroy should use the selected project's exact owner identity");
    assert_eq!(
        runtime.value("__elanu_live$selectedTask").unwrap(),
        Value::String(String::new())
    );
}
