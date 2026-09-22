use crate::runtime::{Runtime, Value};
use crate::check_source_with_runtime_models;

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state documents: [live Document] = []
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state sourceFolder: maybe live Folder = none
state destinationFolder: maybe live Folder = none
state selectedDocument: maybe live Document = none

derived inSource = selectedDocument is in sourceFolder.documents
derived inDestination = selectedDocument is in destinationFolder.documents

action seed {
    create Folder in workspace as left {
        insert left into workspace.folders
    }
    create Folder in workspace as right {
        insert right into workspace.folders
    }

    sourceFolder = workspace.folders[0]
    destinationFolder = workspace.folders[1]

    create Document in sourceFolder as document {
        through document.title = "Draft"
        insert document into sourceFolder.documents
        selectedDocument = document
    }
}

action relocate {
    transfer selectedDocument from sourceFolder to destinationFolder
    insert selectedDocument into destinationFolder.documents
    remove selectedDocument from sourceFolder.documents
}

action relocateThenFail {
    transfer selectedDocument from sourceFolder to destinationFolder
    insert selectedDocument into destinationFolder.documents
    remove selectedDocument from sourceFolder.documents
    fail "abort"
}

action destroyFromSource {
    destroy selectedDocument in sourceFolder
}

action destroyFromDestination {
    destroy selectedDocument in destinationFolder
}
"#;

#[test]
fn dynamic_owner_transfer_insert_and_remove_compose_atomically() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inDestination").unwrap(), Value::Bool(false));

    runtime
        .run_action("relocate")
        .expect("full dynamic-owner move should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inDestination").unwrap(), Value::Bool(true));

    let old_owner = runtime
        .run_action("destroyFromSource")
        .expect_err("old owner must no longer prove lifetime authority");
    assert!(old_owner.message.contains("requires rooting owner"));

    runtime
        .run_action("destroyFromDestination")
        .expect("destination owner should prove lifetime authority");
}

#[test]
fn dynamic_owner_move_rolls_provenance_and_both_memberships_back_together() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    runtime
        .run_action("relocateThenFail")
        .expect_err("later failure should roll the complete move back");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inDestination").unwrap(), Value::Bool(false));

    runtime
        .run_action("destroyFromSource")
        .expect("source owner should remain authoritative after rollback");
}
