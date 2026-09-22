use crate::check_source_with_runtime_models;
use crate::runtime::{Runtime, Value};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model Folder {
    state folders: [live Folder] = []
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state sourceFolder: maybe live Folder = none
state destinationFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none

derived inSource = selectedFolder is in sourceFolder.folders
derived inDestination = selectedFolder is in destinationFolder.folders

action seed {
    create Folder in workspace as left {
        insert left into workspace.folders
    }
    create Folder in workspace as right {
        insert right into workspace.folders
    }
    create Folder in workspace as nested {
        insert nested into workspace.folders
    }

    sourceFolder = workspace.folders[0]
    destinationFolder = workspace.folders[1]
    selectedFolder = workspace.folders[2]
    insert selectedFolder into sourceFolder.folders
}

action moveNested {
    insert selectedFolder into destinationFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action moveNestedThenFail {
    insert selectedFolder into destinationFolder.folders
    remove selectedFolder from sourceFolder.folders
    fail "abort"
}
"#;

#[test]
fn workspace_rooted_nested_folder_moves_without_lifetime_transfer() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inDestination").unwrap(), Value::Bool(false));

    runtime
        .run_action("moveNested")
        .expect("nested structural move should commit without lifetime transfer");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inDestination").unwrap(), Value::Bool(true));
}

#[test]
fn workspace_rooted_nested_folder_move_rolls_back_atomically() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    runtime
        .run_action("moveNestedThenFail")
        .expect_err("later failure should roll both membership edits back");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inDestination").unwrap(), Value::Bool(false));
}
