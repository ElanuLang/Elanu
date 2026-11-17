//! Pressure for structural Up in the real Notes Cabinet.
//!
//! This file deliberately distinguishes two questions:
//!
//! 1. Can an explicit Home-rooted runtime-sized route replay atomically in source?
//!    PR #84 says yes.
//! 2. Can each presentation retain only its own route gesture history and still
//!    implement Up while `selectedFolder` is one shared source-owned selection?
//!
//! The negative cases below are evidence, not selected language law. They pin
//! the exact host/application seam before adding path, parent, occurrence, or
//! navigation-context semantics.

use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = include_str!("../../examples/cabinet.elnu");

#[derive(Debug, Clone, Default)]
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
        backing_replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), RuntimeError> {
        let mut candidate = self.backing.clone();

        for (key, payload) in backing_replacements {
            candidate.insert(key.clone(), payload.clone());
        }

        self.backing = candidate;
        self.manifest = Some(manifest.to_vec());
        Ok(())
    }
}

fn open_cabinet() -> (
    elanu_compiler::CheckedSource,
    PartialPersistentRuntime<MemoryProvider>,
) {
    let checked = check_source_with_runtime_models(SOURCE).expect("cabinet source should check");
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh cabinet should open");

    runtime
        .run_action("initialize")
        .expect("cabinet initialization should commit");

    (checked, runtime)
}

fn selected_folder_name(runtime: &mut PartialPersistentRuntime<MemoryProvider>) -> String {
    match runtime.value("selectedFolderName").unwrap() {
        Value::String(value) => value,
        other => panic!("selectedFolderName produced {other}, expected String"),
    }
}

fn create_two_level_tree(runtime: &mut PartialPersistentRuntime<MemoryProvider>) {
    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .expect("Projects creation should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Archive".into())])
        .expect("Archive creation should commit");

    runtime
        .run_action("selectHome")
        .expect("Home selection should commit");
}

fn replay_home_route(
    runtime: &mut PartialPersistentRuntime<MemoryProvider>,
    route: &[i64],
) -> Result<(), RuntimeError> {
    runtime.run_action_with_values(
        "selectHomeRoute",
        &[Value::ValueSequence(
            route.iter().copied().map(Value::Int).collect(),
        )],
    )
}

#[test]
fn home_route_replay_is_atomic_across_partial_residency() {
    let (checked, mut runtime) = open_cabinet();
    create_two_level_tree(&mut runtime);

    let provider = runtime.into_provider();
    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("cabinet should restart");

    replay_home_route(&mut restarted, &[0, 0])
        .expect("Home -> Projects -> Archive should replay in one action");

    restarted
        .materialize_designation("selectedFolder")
        .expect("selected Archive should explicitly materialize for host observation");

    assert_eq!(selected_folder_name(&mut restarted), "Archive");

    let error = replay_home_route(&mut restarted, &[0, 9])
        .expect_err("later invalid route segment should fail the whole replay");

    assert!(
        error.message.contains("out of bounds"),
        "unexpected route failure: {}",
        error.message
    );

    restarted
        .materialize_designation("selectedFolder")
        .expect("rolled-back selection should remain observable");

    assert_eq!(
        selected_folder_name(&mut restarted),
        "Archive",
        "failed replay must roll selectHome and the earlier valid segment back too"
    );
}

#[test]
fn surface_local_routes_do_not_preserve_shared_arrival_context() {
    let (_, mut runtime) = open_cabinet();
    create_two_level_tree(&mut runtime);

    // The terminal explicitly navigates Home -> Projects and therefore knows
    // only the gesture route it issued.
    let terminal_route = vec![0_i64];
    replay_home_route(&mut runtime, &terminal_route)
        .expect("terminal route should select Projects");
    assert_eq!(selected_folder_name(&mut runtime), "Projects");

    // The browser shares the same source-owned selectedFolder but has no copy of
    // the terminal's presentation history. If it records only its own next
    // gesture, index 0 means "the child I clicked from the current selection";
    // as a Home-rooted route, however, [0] means Projects, not Archive.
    let mut browser_route = Vec::<i64>::new();

    runtime
        .run_action_with_values("selectFolder", &[Value::Int(0)])
        .expect("browser child selection should select Archive");
    browser_route.push(0);

    assert_eq!(selected_folder_name(&mut runtime), "Archive");

    // A generic surface-local Up implementation pops its own route and replays
    // the prefix. That selects Home, even though the actual shared arrival
    // immediately preceding Archive was Projects.
    browser_route.pop();
    replay_home_route(&mut runtime, &browser_route)
        .expect("surface-local route prefix should be executable");

    assert_eq!(
        selected_folder_name(&mut runtime),
        "Notes",
        "surface-local history cannot recover the prior shared Projects context"
    );
}

#[test]
fn source_owned_selection_changes_can_outpace_a_host_gesture_route() {
    let (_, mut runtime) = open_cabinet();
    create_two_level_tree(&mut runtime);

    // Establish a coherent host gesture route to Projects.
    let mut route = vec![0_i64];
    replay_home_route(&mut runtime, &route).expect("route should select Projects");
    assert_eq!(selected_folder_name(&mut runtime), "Projects");

    // createFolder is source-owned application behavior that also selects the
    // fresh child. No structural-index navigation gesture occurred, so the host
    // route still describes Projects rather than the newly selected Drafts.
    runtime
        .run_action_with_values("createFolder", &[Value::String("Drafts".into())])
        .expect("Drafts creation should commit");
    assert_eq!(selected_folder_name(&mut runtime), "Drafts");

    // If the host blindly applies the same "pop current route, replay prefix"
    // rule, it reaches Home. Reaching Projects would require the host to know
    // that createFolder changed selection without adding a route segment, which
    // is Cabinet-specific action knowledge.
    route.pop();
    replay_home_route(&mut runtime, &route).expect("stale route prefix should still execute");

    assert_eq!(
        selected_folder_name(&mut runtime),
        "Notes",
        "gesture-only route state no longer describes the source-owned selection"
    );
}
