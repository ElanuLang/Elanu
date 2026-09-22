use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Workspace {
    state documents: [live Document] = []
    state trash: [live Document] = []
}

state workspace: Workspace
state selected: maybe live Document = none
state recent: maybe live Document = none
state observedRecentTitle = ""
state recentStillPresent = false
state recentStillInDocuments = false
state recentStillInTrash = false

action seed {
    create Document in workspace as document {
        through document.title = "Draft"
        insert document into workspace.documents
    }
    selected = workspace.documents[0]
    recent = workspace.documents[0]
}

action trashSelected {
    remove selected from workspace.documents
    insert selected into workspace.trash
}

action removeEveryMembershipAndClearSelection {
    remove selected from workspace.trash
    selected = none
    recentStillPresent = recent is present
    recentStillInDocuments = recent is in workspace.documents
    recentStillInTrash = recent is in workspace.trash
    observedRecentTitle = recent.title
}
"#;

#[test]
fn removing_all_memberships_and_clearing_one_designation_does_not_end_child_lifetime() {
    let mut runtime = runtime(SOURCE);

    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("trashSelected")
        .expect("trash transition should commit");
    runtime
        .run_action("removeEveryMembershipAndClearSelection")
        .expect("current surface should preserve the still-live child");

    assert_eq!(
        runtime.value("recentStillPresent").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("recentStillInDocuments").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("recentStillInTrash").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("observedRecentTitle").unwrap(),
        Value::String("Draft".into())
    );
}
