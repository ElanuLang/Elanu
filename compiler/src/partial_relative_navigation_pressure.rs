use std::collections::HashMap;

use crate::runtime::persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime};
use crate::runtime::{RuntimeError, Value};

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
state selectedFolder: maybe live Folder = none
state selectedDocument: maybe live Document = none
state thirdDocument: maybe live Document = none

derived selectedTitle = selectedDocument.title

action seed {
    create Folder in workspace as folder {
        insert folder into workspace.folders
    }
    selectedFolder = workspace.folders[0]
    create Document in selectedFolder as first {
        through first.title = "First"
        insert first into selectedFolder.documents
    }
    create Document in selectedFolder as second {
        through second.title = "Second"
        insert second into selectedFolder.documents
    }
    create Document in selectedFolder as third {
        through third.title = "Third"
        insert third into selectedFolder.documents
    }
    selectedDocument = selectedFolder.documents[1]
    thirdDocument = selectedFolder.documents[2]
}

action nextDocument {
    selectedDocument = next selectedDocument in selectedFolder.documents
}

action previousDocument {
    selectedDocument = previous selectedDocument in selectedFolder.documents
}

action clearSelectedDocument {
    selectedDocument = none
}

action duplicateSelectedOccurrence {
    insert selectedDocument into selectedFolder.documents
}

action reorderSelectedToEnd {
    remove selectedDocument from selectedFolder.documents
    insert selectedDocument into selectedFolder.documents
}

action moveSelectedAfterThird {
    move selectedDocument after thirdDocument in selectedFolder.documents
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
}

impl PartialPersistenceProvider for MemoryProvider {
    fn load_manifest(&mut self) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.manifest.clone())
    }

    fn load_backing(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.loads.push(key.to_vec());
        Ok(self.backing.get(key).cloned())
    }

    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        backing_replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), RuntimeError> {
        let mut candidate = self.backing.clone();
        for (key, payload) in backing_replacements {
            candidate.insert(key.clone(), payload.clone());
        }
        self.backing = candidate;
        self.manifest = Some(manifest.to_vec());
        Ok(())
    }
}

fn restarted() -> PartialPersistentRuntime<MemoryProvider> {
    let checked = crate::check_source_with_runtime_models(SOURCE)
        .expect("partial relative-navigation pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    PartialPersistentRuntime::open(checked, provider)
        .expect("restarted partial runtime should open")
}

#[test]
fn relative_reselection_needs_only_owner_backing_then_selected_child_backing() {
    let mut runtime = restarted();

    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("selected Folder should materialize");
    runtime
        .run_action("nextDocument")
        .expect("relative next should resolve from resident ordered structure");
    runtime
        .materialize_designation("selectedDocument")
        .expect("newly selected Document should materialize through designation");

    assert_eq!(
        runtime.value("selectedTitle").unwrap(),
        Value::String("Third".into())
    );
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "only the Folder and newly selected Document should be read"
    );
    assert_ne!(
        provider.loads[0], provider.loads[1],
        "relative navigation should materialize two distinct modeled identities"
    );
}

#[test]
fn relative_boundary_failure_does_not_speculatively_read_child_backing() {
    let mut runtime = restarted();
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .unwrap();
    runtime.run_action("nextDocument").unwrap();

    runtime
        .run_action("nextDocument")
        .expect_err("next at the current boundary should fail");
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "boundary resolution should need only the resident owner structure"
    );
}

#[test]
fn absent_anchor_failure_does_not_speculatively_read_child_backing() {
    let mut runtime = restarted();
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .unwrap();
    runtime.run_action("clearSelectedDocument").unwrap();

    runtime
        .run_action("nextDocument")
        .expect_err("absent relative-navigation anchor should fail");
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "absent-anchor failure should need only the resident owner structure"
    );
}

#[test]
fn duplicate_anchor_failure_does_not_speculatively_read_child_backing() {
    let mut runtime = restarted();
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .unwrap();
    runtime.run_action("duplicateSelectedOccurrence").unwrap();

    runtime
        .run_action("previousDocument")
        .expect_err("duplicate anchor occurrences should remain ambiguous");
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "ambiguity resolution should need only the resident owner structure"
    );
}

#[test]
fn relative_navigation_uses_current_reordered_structure_without_child_reads() {
    let mut runtime = restarted();
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .unwrap();
    runtime
        .run_action("reorderSelectedToEnd")
        .expect("structural remove-plus-insert should reorder resident Folder structure");
    runtime
        .run_action("previousDocument")
        .expect("relative navigation should use the reordered current structure");
    runtime
        .materialize_designation("selectedDocument")
        .expect("newly selected previous Document should materialize");

    assert_eq!(
        runtime.value("selectedTitle").unwrap(),
        Value::String("Third".into())
    );
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "reordering and relative selection should not read dormant sibling backing"
    );
}

#[test]
fn designation_owned_move_reorders_resident_owner_without_loading_children() {
    let mut runtime = restarted();
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("selected Folder should materialize");

    runtime
        .run_action("moveSelectedAfterThird")
        .expect("move should resolve the exact designation-owned documents membership");
    runtime
        .run_action("previousDocument")
        .expect("relative navigation should observe the moved current structure");
    runtime
        .materialize_designation("selectedDocument")
        .expect("newly selected previous Document should materialize");

    assert_eq!(
        runtime.value("selectedTitle").unwrap(),
        Value::String("Third".into())
    );
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "move should need only owner backing; only later designation materialization reads one child"
    );
}
