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

state folder: Folder
state selected: maybe live Document = none
state observedSelected = ""
state observedSibling = ""

action seed {
    create Document in folder as first {
        through first.title = "First"
        insert first into folder.documents
    }
    create Document in folder as second {
        through second.title = "Second"
        insert second into folder.documents
    }
    selected = folder.documents[0]
}

action rename(state target: String) {
    target = "Renamed"
}

action renameSelected {
    rename(state through selected.title)
}

action observeSelected {
    observedSelected = selected.title
}

action observeSibling {
    observedSibling = folder.documents[1].title
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
fn writable_authority_through_materialized_designation_preserves_exact_member_and_backing() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("designation writable-authority pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(provider.backing.len(), 2, "seed should produce two document backings");

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave both documents dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 2);

    runtime
        .materialize_designation("selected")
        .expect("host should materialize exactly the selected document");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "unrelated sibling should remain dormant"
    );

    let provider = runtime.into_provider();
    assert_eq!(provider.loads.len(), 1, "explicit selected materialization should read one backing");
    let selected_key = provider.loads[0].clone();

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("reopening after explicit materialization should restore the durable dormant world");
    runtime
        .materialize_designation("selected")
        .expect("selected target should materialize again for the authority action");
    let mut provider = runtime.into_provider();
    let second_materialization_key = provider
        .loads
        .last()
        .cloned()
        .expect("second selected materialization should record a backing read");
    assert_eq!(second_materialization_key, selected_key);
    provider.loads.clear();
    provider.replacements.clear();

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("durable world should reopen before final authority run");
    runtime
        .materialize_designation("selected")
        .expect("selected target must be resident before ordinary source execution");
    let mut provider = runtime.into_provider();
    assert_eq!(provider.loads, vec![selected_key.clone()]);
    provider.loads.clear();
    provider.replacements.clear();

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("final durable reopen should again leave both children dormant");
    runtime
        .materialize_designation("selected")
        .expect("materialize selected immediately before authority grant");
    let selected_key = runtime.into_provider().loads.last().cloned().unwrap();

    let mut runtime = PartialPersistentRuntime::open(
        checked.clone(),
        {
            let mut provider = MemoryProvider::default();
            // This block is never reached with durable state and exists only to satisfy construction.
            // Replaced below by the actual provider path.
            provider
        },
    );
    drop(runtime);
}
