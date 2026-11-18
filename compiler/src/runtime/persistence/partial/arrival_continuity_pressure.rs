//! Test-only pressure: can a runtime-private witness attest that a previously
//! recorded structural route still denotes the selected exact identity?
//!
//! The witness is deliberately NOT a public host API. It inspects runtime-owned
//! identity and membership only inside this private test module. A production
//! boundary could return a boolean, but the experiment does not select one.
//!
//! This tests current-state coherence. It cannot prove uninterrupted historical
//! occurrence continuity after a remove/reinsert round-trip because Elanu has no
//! stable occurrence identity or mutation-history token.

use super::*;

const CABINET: &str = include_str!("../../../../../examples/cabinet.elnu");
const PRESSURE_SOURCE: &str = r#"
action selectHomeRoute(route: [Int]) {
    selectHome()
    selectFolder(route)
}

state otherFolder: maybe live Folder = none

action addHomeSibling(name: String) {
    create Folder under homeFolder as sibling {
        insert sibling into homeFolder.folders
        otherFolder = sibling
    }
    through otherFolder.name = name
}

action reorderHome {
    move selectedFolder after otherFolder in homeFolder.folders
}

action duplicateSelectedHomeOccurrence {
    insert selectedFolder into homeFolder.folders
}
"#;

#[derive(Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
}

impl PartialPersistenceProvider for MemoryProvider {
    fn load_manifest(&mut self) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.manifest.clone())
    }

    fn load_backing(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.backing.get(key).cloned())
    }

    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), RuntimeError> {
        let mut next = self.backing.clone();
        for (key, value) in replacements {
            next.insert(key.clone(), value.clone());
        }
        self.backing = next;
        self.manifest = Some(manifest.to_vec());
        Ok(())
    }
}

type CabinetRuntime = PartialPersistentRuntime<MemoryProvider>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteEdge {
    // Runtime-private facts, never returned through a host boundary.
    parent_identity: String,
    complete_membership: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteWitness {
    indices: Vec<usize>,
    root_identity: String,
    selected_identity: String,
    edges: Vec<RouteEdge>,
}

/// Test-only internal observation. Compare exact selected identity AND every
/// traversed parent sequence, rather than only a name, selection, or final
/// target. Full sequence comparison is intentionally conservative: a sibling
/// change invalidates even if the selected occurrence did not move.
fn observe(runtime: &mut CabinetRuntime, indices: &[usize]) -> Option<RouteWitness> {
    let (root_identity, _) = runtime.designation_identity_and_handle("homeFolder").ok()?;
    let (selected_identity, _) = runtime
        .designation_identity_and_handle("selectedFolder")
        .ok()?;

    let mut current = root_identity.clone();
    let mut edges = Vec::new();
    for &index in indices {
        let handle = runtime.backed.get(&current)?.clone();
        runtime.materialize(&encode_key(handle.token)).ok()?;
        let values =
            runtime_backing_values(&runtime.checked, &runtime.runtime, &current, &handle).ok()?;
        let Value::Sequence { targets, .. } = values.get("folders")? else {
            return None;
        };
        let next = targets.get(index)?.clone();
        edges.push(RouteEdge {
            parent_identity: current,
            complete_membership: targets.clone(),
        });
        current = next;
    }

    // A route that currently traverses correctly is not coherent if some
    // source action independently changed the source-owned selection.
    if current != selected_identity {
        return None;
    }

    Some(RouteWitness {
        indices: indices.to_vec(),
        root_identity,
        selected_identity,
        edges,
    })
}

fn coherent(runtime: &mut CabinetRuntime, prior: &RouteWitness) -> bool {
    observe(runtime, &prior.indices).is_some_and(|now| now == *prior)
}

fn initialized() -> CabinetRuntime {
    let source = format!("{CABINET}\n{PRESSURE_SOURCE}");
    let checked = crate::check_source_with_runtime_models(&source)
        .expect("Cabinet plus test-only actions must check");
    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default()).unwrap();
    runtime.run_action("initialize").unwrap();
    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .unwrap();
    runtime.run_action("selectHome").unwrap();
    runtime
}

fn replay(runtime: &mut CabinetRuntime, indices: &[i64]) {
    runtime
        .run_action_with_values(
            "selectHomeRoute",
            &[Value::ValueSequence(
                indices.iter().copied().map(Value::Int).collect(),
            )],
        )
        .unwrap();
}

#[test]
fn a_harmless_rename_retains_route_coherence_without_revealing_identity() {
    let mut runtime = initialized();
    replay(&mut runtime, &[0]);
    runtime
        .run_action_with_values("createFolder", &[Value::String("Archive".into())])
        .unwrap();
    replay(&mut runtime, &[0, 0]);
    let witness = observe(&mut runtime, &[0, 0]).expect("known arrival");
    runtime
        .run_action_with_values(
            "renameSelectedFolder",
            &[Value::String("Archive renamed".into())],
        )
        .unwrap();

    assert!(coherent(&mut runtime, &witness));
    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Archive renamed".into())
    );
}

#[test]
fn unknown_reselection_invalidates_even_when_old_route_still_exists() {
    let mut runtime = initialized();
    replay(&mut runtime, &[0]);
    let witness = observe(&mut runtime, &[0]).unwrap();

    runtime.run_action("selectHome").unwrap();
    assert!(!coherent(&mut runtime, &witness));

    replay(&mut runtime, &[0]);
    assert!(coherent(&mut runtime, &witness));
    runtime
        .run_action_with_values("createFolder", &[Value::String("Drafts".into())])
        .unwrap();
    assert!(!coherent(&mut runtime, &witness));
}

#[test]
fn structural_reorder_invalidates_despite_unchanged_selected_identity() {
    let mut runtime = initialized();
    runtime
        .run_action_with_values("addHomeSibling", &[Value::String("Other".into())])
        .unwrap();
    replay(&mut runtime, &[0]);
    let witness = observe(&mut runtime, &[0]).unwrap();
    let selected_before = runtime
        .designation_identity_and_handle("selectedFolder")
        .unwrap()
        .0;

    runtime.run_action("reorderHome").unwrap();

    let selected_after = runtime
        .designation_identity_and_handle("selectedFolder")
        .unwrap()
        .0;
    assert_eq!(selected_before, selected_after);
    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Projects".into())
    );
    assert!(!coherent(&mut runtime, &witness));
}

#[test]
fn failed_transition_preserves_opaque_continuity() {
    let mut runtime = initialized();
    replay(&mut runtime, &[0]);
    let witness = observe(&mut runtime, &[0]).unwrap();

    assert!(runtime
        .run_action_with_values("selectFolder", &[Value::Int(99)])
        .is_err());
    assert!(coherent(&mut runtime, &witness));
}

#[test]
fn duplicate_occurrence_routes_have_distinct_witnesses_even_with_same_target() {
    let mut runtime = initialized();
    replay(&mut runtime, &[0]);
    runtime
        .run_action("duplicateSelectedHomeOccurrence")
        .unwrap();

    replay(&mut runtime, &[0]);
    let first = observe(&mut runtime, &[0]).unwrap();
    replay(&mut runtime, &[1]);
    let second = observe(&mut runtime, &[1]).unwrap();

    assert_eq!(first.selected_identity, second.selected_identity);
    assert_ne!(first.indices, second.indices);
    assert!(coherent(&mut runtime, &first));
    assert!(coherent(&mut runtime, &second));

    // A structural mutation invalidates both witnesses, even though neither
    // selected identity nor its primitive name changes.
    runtime
        .run_action_with_values("addHomeSibling", &[Value::String("Other".into())])
        .unwrap();
    assert!(!coherent(&mut runtime, &first));
    assert!(!coherent(&mut runtime, &second));
}
