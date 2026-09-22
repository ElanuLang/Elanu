use crate::check_source_with_runtime_models;
use crate::runtime::{Runtime, Value};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model Folder {
    state folders: [live Folder] = []
    state restoreParents: [live Folder] = []
}

state workspace: Folder
state sourceFolder: maybe live Folder = none
state trashFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none

derived inSource = selectedFolder is in sourceFolder.folders
derived inTrash = selectedFolder is in trashFolder.folders
derived remembersSource = sourceFolder is in selectedFolder.restoreParents

action seed {
    create Folder in workspace as source {
        insert source into workspace.folders
    }
    create Folder in workspace as trash {
        insert trash into workspace.folders
    }

    sourceFolder = workspace.folders[0]
    trashFolder = workspace.folders[1]

    create Folder in sourceFolder as selected {
        insert selected into sourceFolder.folders
    }
    selectedFolder = sourceFolder.folders[0]
}

action moveToTrash {
    insert sourceFolder into selectedFolder.restoreParents
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action restoreFromTrash {
    restoreDestination = selectedFolder.restoreParents[0]
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    remove restoreDestination from selectedFolder.restoreParents
}

action restoreFromTrashThenFail {
    restoreDestination = selectedFolder.restoreParents[0]
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    remove restoreDestination from selectedFolder.restoreParents
    fail "abort"
}

action proveRestoredOwner {
    transfer selectedFolder from sourceFolder to sourceFolder
}
"#;

#[test]
fn current_structure_can_behaviorally_remember_and_restore_one_parent() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    runtime
        .run_action("moveToTrash")
        .expect("trash move should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("remembersSource").unwrap(), Value::Bool(true));

    runtime
        .run_action("restoreFromTrash")
        .expect("restore should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("remembersSource").unwrap(), Value::Bool(false));

    runtime
        .run_action("proveRestoredOwner")
        .expect("original parent should again prove lifetime provenance");
}

#[test]
fn restore_rolls_back_parent_membership_and_remembered_destination_together() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("moveToTrash")
        .expect("trash move should commit");

    runtime
        .run_action("restoreFromTrashThenFail")
        .expect_err("later failure should roll restore back");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("remembersSource").unwrap(), Value::Bool(true));
}

#[test]
fn model_local_scalar_live_designation_is_not_currently_supported() {
    let source = r#"
state model Folder {
    state restoreParent: maybe live Folder = none
}

state workspace: Folder
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("model-local maybe-live designation should remain unsupported in this pressure test");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("state-model member 'Folder.restoreParent' has unsupported bootstrap type")
    }));
}
