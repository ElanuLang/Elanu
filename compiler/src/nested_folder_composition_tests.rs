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

const TRASH_SOURCE: &str = r#"
state model Folder {
    state folders: [live Folder] = []
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state sourceFolder: maybe live Folder = none
state trashFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state descendantFolder: maybe live Folder = none

derived inSource = selectedFolder is in sourceFolder.folders
derived inTrash = selectedFolder is in trashFolder.folders
derived descendantStillNested = descendantFolder is in selectedFolder.folders

action seedTrashCase {
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

    create Folder in selectedFolder as descendant {
        insert descendant into selectedFolder.folders
    }
    descendantFolder = selectedFolder.folders[0]
}

action moveTreeToTrash {
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action proveMovedFolderOwner {
    transfer selectedFolder from trashFolder to trashFolder
}

action proveDescendantOwner {
    transfer descendantFolder from selectedFolder to selectedFolder
}

action moveTreeToTrashThenFail {
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
    fail "abort"
}
"#;

#[test]
fn parent_owned_nonleaf_folder_moves_to_trash_without_descendant_rewrites() {
    let mut runtime = runtime(TRASH_SOURCE);
    runtime
        .run_action("seedTrashCase")
        .expect("trash case should seed");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(false));
    assert_eq!(
        runtime.value("descendantStillNested").unwrap(),
        Value::Bool(true)
    );

    runtime
        .run_action("moveTreeToTrash")
        .expect("non-leaf folder should move to trash atomically");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(true));
    assert_eq!(
        runtime.value("descendantStillNested").unwrap(),
        Value::Bool(true)
    );

    runtime
        .run_action("proveMovedFolderOwner")
        .expect("trash should now prove the moved folder's lifetime parent");
    runtime
        .run_action("proveDescendantOwner")
        .expect("the descendant should remain rooted in the moved folder");
}

#[test]
fn parent_owned_nonleaf_trash_move_rolls_back_without_descendant_rewrites() {
    let mut runtime = runtime(TRASH_SOURCE);
    runtime
        .run_action("seedTrashCase")
        .expect("trash case should seed");

    runtime
        .run_action("moveTreeToTrashThenFail")
        .expect_err("later failure should roll provenance and membership back");

    assert_eq!(runtime.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("inTrash").unwrap(), Value::Bool(false));
    assert_eq!(
        runtime.value("descendantStillNested").unwrap(),
        Value::Bool(true)
    );

    runtime
        .run_action("proveDescendantOwner")
        .expect("rollback must not disturb descendant provenance");
}
