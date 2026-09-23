use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError,
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

action seed {
    create Folder in workspace as folder {
        insert folder into workspace.folders
    }
    selectedFolder = workspace.folders[0]

    create Document in selectedFolder as document {
        through document.title = "Existing"
        insert document into selectedFolder.documents
    }
    selectedDocument = selectedFolder.documents[0]
}

action destroySelected {
    destroy selectedDocument in selectedFolder
}

action selectFirstDocument {
    selectedDocument = selectedFolder.documents[0]
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
        .expect("dormant-destroy partial-persistence pressure source should check");
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    runtime.run_action("seed").expect("seed should publish");
    (checked, runtime.into_provider())
}

#[test]
fn destroy_dormant_child_uses_metadata_and_resident_parent_without_child_read() {
    let (checked, mut provider) = seeded_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        2,
        "seed should produce one Folder and one Document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open without loading dynamic backing");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        2,
        "Folder and Document should both begin dormant"
    );

    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("selected Folder should materialize");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "only the Document should remain dormant after Folder materialization"
    );

    runtime
        .run_action("destroySelected")
        .expect("destroy should terminate the dormant leaf from identity/provenance metadata");
    runtime
        .materialize_designation("selectedDocument")
        .expect_err("destroy must clear the persistent optional designation");
    assert!(
        runtime.dormant_backing_keys().is_empty(),
        "terminated Document must no longer be represented as live dormant backing"
    );

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "destroy must not load the dormant Document backing"
    );
    let folder_key = provider.loads[0].clone();
    let document_key = backing_before
        .keys()
        .find(|key| **key != folder_key)
        .cloned()
        .expect("Document backing should be distinct from Folder backing");
    assert_eq!(
        provider.backing.get(&document_key),
        backing_before.get(&document_key),
        "opaque old Document bytes may remain physically present but must remain untouched"
    );
    assert_eq!(
        provider.replacements.last(),
        Some(&vec![folder_key.clone()]),
        "publication should rewrite only the resident Folder membership"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("post-destroy manifest should restart without child backing reads");
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        1,
        "only the surviving Folder should remain as dormant backed identity"
    );
    restarted
        .materialize_designation("selectedDocument")
        .expect_err("cleared designation must remain absent after restart");
    restarted
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("surviving Folder should still materialize");
    restarted
        .run_action("selectFirstDocument")
        .expect_err("destroyed child occurrence must remain absent from Folder membership");

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![folder_key],
        "post-destroy verification should load only the surviving Folder"
    );
    assert_eq!(
        provider.backing.get(&document_key),
        backing_before.get(&document_key),
        "unreachable provider bytes are not proof of a live modeled identity"
    );
}

#[test]
fn rejected_dormant_destroy_preserves_prior_world_and_can_retry_without_child_read() {
    let (checked, mut provider) = seeded_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    provider.reject_next = true;

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open prior durable world");
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("Folder should materialize before destroy attempt");
    runtime
        .run_action("destroySelected")
        .expect_err("provider rejection must reject lifetime termination atomically");

    let mut provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert_eq!(
        provider.loads.len(),
        1,
        "rejected destroy must not read dormant child backing"
    );
    assert!(
        provider.replacements.is_empty(),
        "rejected provider candidate must not be recorded as accepted replacement"
    );

    provider.loads.clear();
    let mut retried = PartialPersistentRuntime::open(checked, provider)
        .expect("prior durable world should remain restartable after rejection");
    assert_eq!(
        retried.dormant_backing_keys().len(),
        2,
        "rejected destroy must leave both Folder and Document live/backed"
    );
    retried
        .materialize_root_member_index("workspace", "folders", 0)
        .unwrap();
    retried
        .run_action("destroySelected")
        .expect("same dormant-child destroy should succeed on retry");
    assert!(
        retried.dormant_backing_keys().is_empty(),
        "successful retry must retire the child backing identity while Folder stays resident"
    );

    let provider = retried.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "successful retry should still require only the Folder backing read"
    );
}