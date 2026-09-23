use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
    CheckedSource,
};

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
    }
    selectedFolder = workspace.folders[0]

    create Document in selectedFolder as existing {
        through existing.title = "Existing"
        insert existing into selectedFolder.documents
    }
}

action createAfterRestart {
    create Document in selectedFolder as document {
        through document.title = "Created after restart"
        insert document into selectedFolder.documents
        selectedDocument = document
    }
}

action previousDocument {
    selectedDocument = previous selectedDocument in selectedFolder.documents
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
    replacements: Vec<Vec<Vec<u8>>>,
    reject_next: bool,
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
        if self.reject_next {
            self.reject_next = false;
            return Err(RuntimeError::new("provider rejected candidate"));
        }

        let mut candidate = self.backing.clone();
        for (key, payload) in backing_replacements {
            candidate.insert(key.clone(), payload.clone());
        }
        self.backing = candidate;
        self.manifest = Some(manifest.to_vec());
        self.replacements.push(
            backing_replacements
                .iter()
                .map(|(key, _)| key.clone())
                .collect(),
        );
        Ok(())
    }
}

fn seeded_provider() -> (CheckedSource, MemoryProvider) {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("fresh-child partial-persistence pressure source should check");
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    runtime.run_action("seed").expect("seed should publish");
    (checked, runtime.into_provider())
}

#[test]
fn fresh_child_after_restart_publishes_only_changed_parent_and_new_child() {
    let (checked, mut provider) = seeded_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        2,
        "seed should have one Folder backing and one existing Document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open without loading dynamic backing");
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("selected Folder should materialize");
    runtime
        .run_action("createAfterRestart")
        .expect("fresh Document creation should publish from the resident selected Folder");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "creating the new child should read only the selected Folder backing"
    );
    let folder_key = provider.loads[0].clone();
    let old_document_key = backing_before
        .keys()
        .find(|key| **key != folder_key)
        .cloned()
        .expect("seeded existing Document backing should remain distinct from Folder backing");
    let new_document_key = provider
        .backing
        .keys()
        .find(|key| !backing_before.contains_key(*key))
        .cloned()
        .expect("fresh Document should receive one new opaque backing key");

    assert_eq!(provider.backing.len(), 3);
    assert_eq!(
        provider.backing.get(&old_document_key),
        backing_before.get(&old_document_key),
        "unrelated dormant Document backing must remain byte-for-byte unchanged"
    );
    let replacements = provider
        .replacements
        .last()
        .expect("successful fresh-child publication should record replacements");
    assert_eq!(
        replacements.len(),
        2,
        "publication should replace exactly the changed Folder and the new Document"
    );
    assert!(replacements.contains(&folder_key));
    assert!(replacements.contains(&new_document_key));
    assert!(!replacements.contains(&old_document_key));

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("second restart should accept the fresh identity and its backing metadata");
    restarted
        .materialize_designation("selectedDocument")
        .expect("persisted fresh Document designation should materialize exact new child");
    assert_eq!(
        restarted.value("selectedTitle").unwrap(),
        Value::String("Created after restart".into())
    );

    restarted
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("Folder should materialize with the newly published membership occurrence");
    restarted
        .run_action("previousDocument")
        .expect("fresh child should remain the appended neighbor of the existing Document");
    restarted
        .materialize_designation("selectedDocument")
        .expect("explicit navigation back to existing Document should materialize it");
    assert_eq!(
        restarted.value("selectedTitle").unwrap(),
        Value::String("Existing".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![new_document_key, folder_key, old_document_key],
        "each backing should be read only when that exact identity is explicitly needed"
    );
}

#[test]
fn rejected_fresh_child_publication_preserves_prior_world_and_can_retry() {
    let (checked, mut provider) = seeded_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    provider.reject_next = true;

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider).unwrap();
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("selected Folder should materialize before creation attempt");
    runtime
        .run_action("createAfterRestart")
        .expect_err("provider rejection should reject fresh identity publication atomically");
    runtime
        .materialize_designation("selectedDocument")
        .expect_err("rejected candidate must not publish the fresh selectedDocument identity");

    let mut provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert_eq!(
        provider.loads.len(),
        1,
        "rejected creation should still have read only the selected Folder backing"
    );
    assert!(provider.replacements.is_empty());

    provider.loads.clear();
    let mut retried = PartialPersistentRuntime::open(checked, provider)
        .expect("prior durable world should remain restartable after rejection");
    retried
        .materialize_root_member_index("workspace", "folders", 0)
        .unwrap();
    retried
        .run_action("createAfterRestart")
        .expect("same fresh-child action should succeed on retry");
    retried
        .materialize_designation("selectedDocument")
        .expect("successful retry should publish a materializable fresh child");
    assert_eq!(
        retried.value("selectedTitle").unwrap(),
        Value::String("Created after restart".into())
    );

    let provider = retried.into_provider();
    assert_eq!(provider.backing.len(), backing_before.len() + 1);
}
