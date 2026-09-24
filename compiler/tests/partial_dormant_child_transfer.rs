use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
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
state sourceFolder: maybe live Folder = none
state destinationFolder: maybe live Folder = none
state selectedDocument: maybe live Document = none
state observedTitle = ""

action seed {
    create Folder in workspace as source {
        insert source into workspace.folders
    }
    create Folder in workspace as destination {
        insert destination into workspace.folders
    }
    sourceFolder = workspace.folders[0]
    destinationFolder = workspace.folders[1]

    create Document in sourceFolder as document {
        through document.title = "Dormant transfer"
        insert document into sourceFolder.documents
    }
    selectedDocument = sourceFolder.documents[0]
}

action moveDocument {
    transfer selectedDocument from sourceFolder to destinationFolder
    insert selectedDocument into destinationFolder.documents
    remove selectedDocument from sourceFolder.documents
}

action proveDestinationOwner {
    transfer selectedDocument from destinationFolder to destinationFolder
}

action proveSourceOwner {
    transfer selectedDocument from sourceFolder to sourceFolder
}

action selectFromSource {
    selectedDocument = sourceFolder.documents[0]
}

action selectFromDestination {
    selectedDocument = destinationFolder.documents[0]
}

action readSelectedTitle {
    observedTitle = selectedDocument.title
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
    replacements: Vec<Vec<Vec<u8>>>,
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
        self.replacements.push(
            backing_replacements
                .iter()
                .map(|(key, _)| key.clone())
                .collect(),
        );
        Ok(())
    }
}

#[test]
fn transfer_dormant_child_uses_provenance_metadata_without_loading_child_backing() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant-transfer partial-persistence pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        3,
        "seed should produce two Folder backings and one Document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open without dynamic backing reads");
    assert_eq!(runtime.dormant_backing_keys().len(), 3);

    runtime
        .materialize_designation("sourceFolder")
        .expect("source Folder should materialize");
    runtime
        .materialize_designation("destinationFolder")
        .expect("destination Folder should materialize");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "only the Document should remain dormant before transfer"
    );

    runtime
        .run_action("moveDocument")
        .expect("dormant child should transfer using exact provenance metadata");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "transfer plus resident membership edits must not load the dormant Document"
    );
    let document_key = backing_before
        .keys()
        .find(|key| !provider.loads.contains(key))
        .cloned()
        .expect("one untouched backing key should belong to the dormant Document");
    assert_eq!(
        provider.backing.get(&document_key),
        backing_before.get(&document_key),
        "provenance transfer must not rewrite unchanged Document member backing"
    );
    let replacements = provider
        .replacements
        .last()
        .expect("successful transfer action should publish a candidate");
    assert_eq!(
        replacements.len(),
        2,
        "only the two resident Folder memberships should need backing replacement"
    );
    assert!(
        !replacements.contains(&document_key),
        "Document provenance belongs to runtime-owned metadata, not its member payload"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("transferred world should restart without dynamic backing reads");
    assert_eq!(restarted.dormant_backing_keys().len(), 3);

    restarted
        .run_action("proveDestinationOwner")
        .expect("new exact owner should prove provenance without materializing child state");
    restarted
        .run_action("proveSourceOwner")
        .expect_err("old owner must no longer prove the transferred child's provenance");

    restarted
        .materialize_designation("sourceFolder")
        .expect("source Folder should materialize for membership verification");
    restarted
        .materialize_designation("destinationFolder")
        .expect("destination Folder should materialize for membership verification");
    restarted
        .run_action("selectFromSource")
        .expect_err("source membership should no longer contain the transferred Document");
    restarted
        .run_action("selectFromDestination")
        .expect("destination membership should contain the transferred Document");

    let provider_loads_before_child = restarted.dormant_backing_keys().len();
    assert_eq!(
        provider_loads_before_child, 1,
        "Document should still be the only dormant identity before explicit materialization"
    );
    restarted
        .materialize_designation("selectedDocument")
        .expect("transferred Document should remain independently materializable");
    restarted
        .run_action("readSelectedTitle")
        .expect("materialized Document state should remain intact");
    assert_eq!(
        restarted.value("observedTitle").unwrap(),
        Value::String("Dormant transfer".into())
    );

    let provider = restarted.into_provider();
    assert!(
        provider.loads.contains(&document_key),
        "Document backing should be read only when explicitly materialized after transfer proof"
    );
}
