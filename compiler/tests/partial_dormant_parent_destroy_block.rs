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

const STAGED_TRANSFER_OUT_SOURCE: &str = r#"
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

    create Document in selectedChild as document {
        through document.title = "Transferred descendant"
        insert document into selectedChild.documents
    }
    selectedDocument = selectedChild.documents[0]
}

action transferOutThenDestroy {
    transfer selectedDocument from selectedChild to destinationFolder
    destroy selectedChild in sourceFolder
}

action proveDestinationOwnsDocument {
    transfer selectedDocument from destinationFolder to destinationFolder
}

action proveSourceOwnsChild {
    transfer selectedChild from sourceFolder to sourceFolder
}

action selectSourceChild {
    selectedChild = sourceFolder.folders[0]
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

#[test]
fn staged_descendant_transfer_out_allows_dormant_parent_destroy() {
    let checked = check_source_with_runtime_models(STAGED_TRANSFER_OUT_SOURCE)
        .expect("staged transfer-out destroy pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        4,
        "seed should produce source, destination, child, and document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all four modeled identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("transferOutThenDestroy")
        .expect("staged transfer out must remove the descendant blocker before destroy");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        3,
        "destroyed child must disappear from the live dormant identity set"
    );

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "accepted destroy should need only the dormant source membership backing for established cleanup"
    );
    let source_cleanup_key = provider.loads[0].clone();
    assert_eq!(provider.replacements.len(), 1);
    assert_eq!(
        provider.replacements[0],
        vec![source_cleanup_key.clone()],
        "publication should rewrite only the dormant source backing whose membership contained the terminated child"
    );
    assert_eq!(
        provider.backing.len(),
        4,
        "physical opaque bytes may remain even though the child is no longer a live manifested identity"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted transfer-and-destroy candidate should restart");
    restarted
        .run_action("proveDestinationOwnsDocument")
        .expect("destination must own the transferred dormant document after restart");
    restarted
        .run_action("proveSourceOwnsChild")
        .expect_err("terminated child designation must no longer prove a live child");

    restarted
        .materialize_designation("sourceFolder")
        .expect("source Folder should remain materializable");
    let error = restarted
        .run_action("selectSourceChild")
        .expect_err("terminated child occurrence must be absent from source membership");
    assert!(
        error.message.contains("out of bounds"),
        "source membership should be empty after cleanup: {}",
        error.message
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![source_cleanup_key.clone()],
        "the only backing read so far should be the source Folder later materialized explicitly"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(STAGED_TRANSFER_OUT_SOURCE)
            .expect("transfer-out source should still check"),
        provider,
    )
    .expect("source materialization should leave the accepted world restartable");
    restarted
        .materialize_designation("selectedDocument")
        .expect("transferred document should remain independently materializable");
    restarted
        .run_action("readDocumentTitle")
        .expect("transferred document payload should remain intact");
    assert_eq!(
        restarted.value("observedTitle").unwrap(),
        Value::String("Transferred descendant".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads.len(), 2);
    assert_ne!(
        provider.loads[1], source_cleanup_key,
        "document materialization must use its own backing, proving the destroy action did not read it"
    );
}
