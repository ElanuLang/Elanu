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

derived selectedTitle = selectedDocument.title

action seed {
    create Folder in workspace as folder {
        insert folder into workspace.folders
        create Document in folder as first {
            through first.title = "First"
            insert first into folder.documents
        }
        create Document in folder as second {
            through second.title = "Second"
            insert second into folder.documents
        }
    }
    selectedFolder = workspace.folders[0]
}

action selectSecondDocument {
    selectedDocument = selectedFolder.documents[1]
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

#[test]
fn existing_bridges_compose_for_designation_rooted_nested_navigation() {
    let checked = crate::check_source_with_runtime_models(SOURCE)
        .expect("nested partial-navigation pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("restarted partial runtime should open");

    restarted
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("root structural selection should materialize only the selected Folder");
    restarted
        .run_action("selectSecondDocument")
        .expect("resident designated Folder should expose its exact nested Document identity");
    restarted
        .materialize_designation("selectedDocument")
        .expect(
            "selected nested Document should materialize through the existing designation bridge",
        );

    assert_eq!(
        restarted.value("selectedTitle").unwrap(),
        Value::String("Second".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "only the selected Folder and selected Document should be read"
    );
    assert_ne!(
        provider.loads[0], provider.loads[1],
        "nested navigation should materialize two distinct modeled identities"
    );
}
