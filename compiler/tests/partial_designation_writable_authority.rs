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
    assert_eq!(
        provider.backing.len(),
        2,
        "seed should produce two document backings"
    );

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

    runtime
        .run_action("renameSelected")
        .expect("writable authority should target the already-materialized selected member");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only explicit selected materialization should read dynamic backing"
    );
    let selected_key = provider.loads[0].clone();
    assert_eq!(
        provider.replacements,
        vec![vec![selected_key.clone()]],
        "accepted authority mutation should replace only selected backing"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("published authority mutation should restart with both documents dormant");
    assert_eq!(restarted.dormant_backing_keys().len(), 2);

    restarted
        .materialize_designation("selected")
        .expect("selected target should remain materializable after authority mutation");
    restarted
        .run_action("observeSelected")
        .expect("selected title should be readable after restart");
    assert_eq!(
        restarted.value("observedSelected").unwrap(),
        Value::String("Renamed".into())
    );

    restarted
        .materialize_root_member_index("folder", "documents", 1)
        .expect("unrelated sibling should remain independently materializable");
    restarted
        .run_action("observeSibling")
        .expect("sibling title should remain readable after restart");
    assert_eq!(
        restarted.value("observedSibling").unwrap(),
        Value::String("Second".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "verification should materialize exactly selected child and sibling"
    );
    assert_eq!(provider.loads[0], selected_key);
    assert_ne!(
        provider.loads[1], selected_key,
        "sibling must retain a distinct backing identity"
    );
}
