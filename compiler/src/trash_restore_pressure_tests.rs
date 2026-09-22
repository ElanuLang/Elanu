use crate::check_source_with_runtime_models;
use crate::runtime::{Runtime, Value};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model Folder {
    state folders: [live Folder] = []
    state restoreParent: maybe live Folder = none
}

state workspace: Folder
state sourceFolder: maybe live Folder = none
state trashFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none

derived inSource = selectedFolder is in sourceFolder.folders
derived inTrash = selectedFolder is in trashFolder.folders

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
    through selectedFolder.restoreParent = sourceFolder
    restoreDestination = selectedFolder.restoreParent
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action inspectRememberedParent {
    restoreDestination = selectedFolder.restoreParent
}

action restoreFromTrash {
    restoreDestination = selectedFolder.restoreParent
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    through selectedFolder.restoreParent = none
}

action restoreFromTrashThenFail {
    restoreDestination = selectedFolder.restoreParent
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    through selectedFolder.restoreParent = none
    fail "abort"
}

action destroyRememberedSource {
    destroy sourceFolder in workspace
}

action proveRestoredOwner {
    transfer selectedFolder from sourceFolder to sourceFolder
}
"#;

fn designation(runtime: &mut Runtime, name: &str) -> String {
    let Value::String(target) = runtime
        .value(&format!("__meld_live${name}"))
        .expect("designation should exist")
    else {
        panic!("designation carrier should be String");
    };
    target
}

#[test]
fn model_local_optional_designation_remembers_and_restores_one_parent() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let source = designation(&mut runtime, "sourceFolder");

    runtime
        .run_action("moveToTrash")
        .expect("trash move should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(true));
    assert_eq!(designation(&mut runtime, "restoreDestination"), source);

    runtime
        .run_action("restoreFromTrash")
        .expect("restore should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(false));

    runtime
        .run_action("inspectRememberedParent")
        .expect("cleared model-local designation should copy as none");
    assert!(designation(&mut runtime, "restoreDestination").is_empty());

    runtime
        .run_action("proveRestoredOwner")
        .expect("original parent should again prove lifetime provenance");
}

#[test]
fn restore_rolls_back_model_local_designation_and_structure_together() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let source = designation(&mut runtime, "sourceFolder");
    runtime
        .run_action("moveToTrash")
        .expect("trash move should commit");

    runtime
        .run_action("restoreFromTrashThenFail")
        .expect_err("later failure should roll restore back");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(true));
    runtime
        .run_action("inspectRememberedParent")
        .expect("remembered parent should survive rollback");
    assert_eq!(designation(&mut runtime, "restoreDestination"), source);
}

#[test]
fn target_lifetime_end_clears_model_local_optional_designation() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("moveToTrash")
        .expect("trash move should commit");

    runtime
        .run_action("destroyRememberedSource")
        .expect("remembered source should now be a destroyable leaf");
    assert!(designation(&mut runtime, "sourceFolder").is_empty());

    runtime
        .run_action("inspectRememberedParent")
        .expect("lifetime cleanup should have cleared the model-local slot");
    assert!(designation(&mut runtime, "restoreDestination").is_empty());
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(true));
}

#[test]
fn model_local_plain_live_designation_remains_unselected() {
    let source = r#"
state model Folder {
    state restoreParent: live Folder = none
}

state workspace: Folder
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("model-local plain-live designation should remain unsupported");
    assert!(!errors.is_empty());
}

#[test]
fn wrong_model_assignment_to_model_local_designation_is_rejected() {
    let source = r#"
state model Folder {
    state restoreParent: maybe live Folder = none
}

state model Document {
    state title = ""
}

state folderRoot: Folder
state documentRoot: Document
state selectedFolder: maybe live Folder = live folderRoot
state selectedDocument: maybe live Document = live documentRoot

action bad {
    through selectedFolder.restoreParent = selectedDocument
}
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("wrong-model designation assignment should fail");
    assert!(errors.iter().any(|error| {
        error.message.contains("slot requires maybe live Folder")
            || error.message.contains("requires maybe live Folder")
    }));
}

#[test]
fn model_local_designation_is_not_an_ordinary_string_value() {
    let source = r#"
state model Folder {
    state restoreParent: maybe live Folder = none
}

state folderRoot: Folder
state selectedFolder: maybe live Folder = live folderRoot
state leaked = ""

action bad {
    leaked = selectedFolder.restoreParent
}
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("designation carrier must not leak into ordinary value expressions");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("model-local maybe live Folder designation")
            && error.message.contains("not an ordinary value")
    }));
}
