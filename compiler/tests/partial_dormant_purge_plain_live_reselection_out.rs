use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = r#"
state model Node {
    state name = ""
    state children: [live Node] = []
}

state left: Node
state right: Node
state fallback: Node
state root: maybe live Node = none
state child: maybe live Node = none
state grandchild: maybe live Node = none
state foreign: maybe live Node = none
state pinned: live Node = live fallback
state observedName = ""

action seed {
    create Node in left as rootNode {
        through rootNode.name = "Root"
        insert rootNode into left.children
    }
    root = left.children[0]

    create Node in root as childNode {
        through childNode.name = "Child"
        insert childNode into root.children
    }
    child = root.children[0]

    create Node in child as grandchildNode {
        through grandchildNode.name = "Grandchild"
        insert grandchildNode into child.children
    }
    grandchild = child.children[0]
    pinned = child.children[0]

    create Node in right as foreignNode {
        through foreignNode.name = "Foreign survivor"
        insert foreignNode into right.children
    }
    foreign = right.children[0]
}

action reselectOutThenPurge {
    pinned = right.children[0]
    purge root in left
}

action readPinnedName {
    observedName = pinned.name
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
fn staged_plain_live_reselection_out_allows_dormant_subtree_purge() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant purge plain-live reselection-out source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "seed should produce root, child, grandchild, and foreign backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all four dynamic identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("reselectOutThenPurge")
        .expect("staged plain-live reselection out must remove the purge blocker");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "purge should terminate root, child, and grandchild while foreign survives"
    );

    let mut provider = runtime.into_provider();
    assert!(
        provider.loads.is_empty(),
        "designation reselection and purge should require no dynamic backing reads"
    );
    assert_eq!(
        provider.replacements,
        vec![Vec::<Vec<u8>>::new()],
        "accepted publication should require no dynamic backing replacement"
    );
    assert_eq!(
        provider.backing.len(),
        4,
        "terminated opaque backing may remain physically present"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted reselection-and-purge world should restart");
    assert_eq!(restarted.dormant_backing_keys().len(), 1);
    restarted
        .materialize_designation("pinned")
        .expect("published plain-live designation should now target the surviving foreign node");
    restarted
        .run_action("readPinnedName")
        .expect("reselected plain-live designation should read the surviving target");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Foreign survivor".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only later explicit materialization of the reselected target should read backing"
    );
}
