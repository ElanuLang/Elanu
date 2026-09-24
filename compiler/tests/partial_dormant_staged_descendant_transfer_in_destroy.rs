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
    state name = ""
    state folders: [live Folder] = []
    state documents: [live Document] = []
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state sourceFolder: maybe live Folder = none
state destinationFolder: maybe live Folder = none
state selectedChild: maybe live Folder = none
state selectedDocument: maybe live Document = none
state observedName = ""
state observedTitle = ""

action seed {
    create Folder in workspace as source {
        through source.name = "Source"
        insert source into workspace.folders
    }
    create Folder in workspace as destination {
        through destination.name = "Destination"
        insert destination into workspace.folders
    }
    sourceFolder = workspace.folders[0]
    destinationFolder = workspace.folders[1]

    create Folder in sourceFolder as child {
        through child.name = "Child"
        insert child into sourceFolder.folders
    }
    selectedChild = sourceFolder.folders[0]

    create Document in destinationFolder as document {
        through document.title = "External descendant"
        insert document into destinationFolder.documents
    }
    selectedDocument = destinationFolder.documents[0]
}

action transferInThenDestroy {
    transfer selectedDocument from destinationFolder to selectedChild
    destroy selectedChild in sourceFolder
}

action proveSourceOwnsChild {
    transfer selectedChild from sourceFolder to sourceFolder
}

action proveDestinationOwnsDocument {
    transfer selectedDocument from destinationFolder to destinationFolder
}

action readChildName {
    observedName = selectedChild.name
}

action readDocumentTitle {
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
fn staged_descendant_transfer_in_blocks_dormant_parent_destroy_and_rolls_back() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("staged transfer-in destroy pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        4,
        "seed should produce source, destination, child, and document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all modeled identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    let error = runtime
        .run_action("transferInThenDestroy")
        .expect_err("staged descendant transfer into the child must newly block destroy");
    assert!(
        error.message.contains("roots another live child"),
        "failure should be the established rooted-descendant diagnostic: {}",
        error.message
    );

    let mut provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert!(
        provider.loads.is_empty(),
        "staged descendant blocking should use transaction-visible provenance without member reads"
    );
    assert!(
        provider.replacements.is_empty(),
        "semantic failure must not attempt durable publication"
    );

    provider.loads.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("failed action must leave the prior committed provenance world restartable");
    restarted
        .run_action("proveSourceOwnsChild")
        .expect("rollback must preserve source as the child's exact owner");
    restarted
        .run_action("proveDestinationOwnsDocument")
        .expect("rollback must preserve destination as the document's exact owner");

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-failure provenance proofs should remain metadata-only"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("metadata proofs should leave the world restartable");
    restarted
        .materialize_designation("selectedChild")
        .expect("child should remain independently materializable after rollback");
    restarted
        .run_action("readChildName")
        .expect("child stored state should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Child".into())
    );

    restarted
        .materialize_designation("selectedDocument")
        .expect("document should remain independently materializable after rollback");
    restarted
        .run_action("readDocumentTitle")
        .expect("document stored state should remain intact");
    assert_eq!(
        restarted.value("observedTitle").unwrap(),
        Value::String("External descendant".into())
    );
}
