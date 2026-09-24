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
state selectedChild: maybe live Folder = none
state selectedDocument: maybe live Document = none
state observedName = ""
state observedTitle = ""

action seed {
    create Folder in workspace as source {
        through source.name = "Source"
        insert source into workspace.folders
    }
    sourceFolder = workspace.folders[0]

    create Folder in sourceFolder as child {
        through child.name = "Child"
        insert child into sourceFolder.folders
    }
    selectedChild = sourceFolder.folders[0]

    create Document in selectedChild as document {
        through document.title = "Descendant"
        insert document into selectedChild.documents
    }
    selectedDocument = selectedChild.documents[0]
}

action destroyChild {
    destroy selectedChild in sourceFolder
}

action proveSourceStillOwnsChild {
    transfer selectedChild from sourceFolder to sourceFolder
}

action proveChildStillOwnsDocument {
    transfer selectedDocument from selectedChild to selectedChild
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
fn dormant_live_descendant_blocks_parent_destroy_without_member_backing_reads() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant parent-destroy pressure source should check");
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
        3,
        "seed should produce source Folder, child Folder, and descendant Document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the provenance chain dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 3);

    let error = runtime
        .run_action("destroyChild")
        .expect_err("a dormant live descendant must block non-cascading parent destruction");
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
        "descendant blocking should use persisted provenance metadata without member reads"
    );
    assert!(
        provider.replacements.is_empty(),
        "semantic descendant blocking must not attempt durable publication"
    );

    provider.loads.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("failed destroy must leave the prior provenance world restartable");
    restarted
        .run_action("proveSourceStillOwnsChild")
        .expect("source must still prove the child's lifetime provenance");
    restarted
        .run_action("proveChildStillOwnsDocument")
        .expect("child must still prove the descendant's lifetime provenance");

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-failure provenance proofs should remain metadata-only"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("provenance proofs should leave the world restartable");
    restarted
        .materialize_designation("selectedChild")
        .expect("blocked child should remain independently materializable");
    restarted
        .run_action("readChildName")
        .expect("child stored state should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Child".into())
    );

    restarted
        .materialize_designation("selectedDocument")
        .expect("descendant should remain independently materializable");
    restarted
        .run_action("readDocumentTitle")
        .expect("descendant stored state should remain intact");
    assert_eq!(
        restarted.value("observedTitle").unwrap(),
        Value::String("Descendant".into())
    );
}
