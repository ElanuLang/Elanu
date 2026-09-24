use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = r#"
state model Folder {
    state name = ""
    state folders: [live Folder] = []
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state sourceFolder: maybe live Folder = none
state selectedChild: maybe live Folder = none
state selectedGrandchild: maybe live Folder = none
state observedName = ""

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

    create Folder in selectedChild as grandchild {
        through grandchild.name = "Grandchild"
        insert grandchild into selectedChild.folders
    }
    selectedGrandchild = selectedChild.folders[0]
}

action attemptCycle {
    transfer selectedChild from sourceFolder to selectedGrandchild
}

action proveSourceStillOwnsChild {
    transfer selectedChild from sourceFolder to sourceFolder
}

action proveChildStillOwnsGrandchild {
    transfer selectedGrandchild from selectedChild to selectedChild
}

action readSelectedName {
    observedName = selectedChild.name
}

action readGrandchildName {
    observedName = selectedGrandchild.name
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
fn dormant_provenance_cycle_is_rejected_without_member_backing_reads() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant provenance-cycle pressure source should check");
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
        "seed should produce source, child, and grandchild Folder backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open with the full provenance chain dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 3);

    let error = runtime
        .run_action("attemptCycle")
        .expect_err("moving the child beneath its descendant must be rejected");
    assert!(
        error.message.contains("would create an owner cycle"),
        "failure should be the established provenance-cycle diagnostic: {}",
        error.message
    );

    let mut provider = runtime.into_provider();
    assert!(
        provider.loads.is_empty(),
        "cycle validation should walk persisted provenance metadata without loading member backing"
    );
    assert!(
        provider.replacements.is_empty(),
        "semantic cycle rejection must not attempt durable publication"
    );
    assert_eq!(
        provider.backing, backing_before,
        "failed cycle attempt must leave opaque backing untouched"
    );

    provider.loads.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("failed cycle attempt must leave prior provenance world restartable");
    restarted
        .run_action("proveSourceStillOwnsChild")
        .expect("source must remain the child's exact owner after cycle rejection");
    restarted
        .run_action("proveChildStillOwnsGrandchild")
        .expect("child must remain the grandchild's exact owner after cycle rejection");

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-failure provenance proofs should not materialize dormant member state"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("provenance proofs should leave world restartable");
    restarted
        .materialize_designation("selectedChild")
        .expect("child should remain independently materializable");
    restarted
        .run_action("readSelectedName")
        .expect("child stored state should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Child".into())
    );

    restarted
        .materialize_designation("selectedGrandchild")
        .expect("grandchild should remain independently materializable");
    restarted
        .run_action("readGrandchildName")
        .expect("grandchild stored state should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Grandchild".into())
    );
}
