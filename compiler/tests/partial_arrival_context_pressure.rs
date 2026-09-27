//! Pressure experiment: structural arrival context without intrinsic parenthood.
//!
//! This experiment deliberately adds no Cabinet production source and no runtime
//! mechanism. It asks what existing route/selection semantics can already say
//! about structural "Up" when:
//!
//! - a host knows the occurrence route by which it arrived;
//! - the same exact modeled identity can occur through more than one structural path;
//! - lifetime/rooting provenance is not structural parenthood.
//!
//! A route is test input representing a transient navigation gesture. Nothing here
//! promotes route indices into modeled identity or stable occurrence identity.

use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
    CheckedSource,
};

const CABINET: &str = include_str!("../../examples/cabinet.elnu");

/// Test-only route and ambiguity setup.
///
/// `selectRoute` replays a bounded route from Home. Supplying a shorter depth is
/// therefore equivalent to replaying a known arrival-route prefix.
///
/// `seedAmbiguousArrival` constructs:
///
/// ```text
/// Notes
/// ├── Left
/// │   └── Shared
/// └── Right
///     └── Shared
/// ```
///
/// Both occurrences designate the same exact `Shared` identity. `Shared` is
/// created/rooted under Left, then that same designation is also inserted into
/// Right.folders. Therefore lifetime provenance cannot honestly answer which
/// structural context was used to arrive at Shared.
const PRESSURE: &str = r#"
state leftArrivalFolder: maybe live Folder = none
state rightArrivalFolder: maybe live Folder = none
state sharedArrivalFolder: maybe live Folder = none

action selectRoute(first: Int, second: Int, depth: Int) {
    selectHome()

    if depth == 1 {
        selectFolder(first)
    }

    if depth == 2 {
        selectFolder(first)
        selectFolder(second)
    }
}

action seedAmbiguousArrival {
    selectHome()
    createFolder("Left")
    leftArrivalFolder = selectedFolder

    selectHome()
    createFolder("Right")
    rightArrivalFolder = selectedFolder

    selectedFolder = leftArrivalFolder
    createFolder("Shared")
    sharedArrivalFolder = selectedFolder

    insert sharedArrivalFolder into rightArrivalFolder.folders

    selectHome()
}
action removeLeftFromHome {
    selectHome()
    remove leftArrivalFolder from selectedFolder.folders
}

action removeSharedFromLeft {
    selectedFolder = leftArrivalFolder
    remove sharedArrivalFolder from selectedFolder.folders
    selectHome()
}
"#;

fn source() -> String {
    format!("{CABINET}\n{PRESSURE}")
}

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
    commits: usize,
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
        self.commits += 1;
        Ok(())
    }
}

type CabinetRuntime = PartialPersistentRuntime<MemoryProvider>;

fn checked() -> CheckedSource {
    check_source_with_runtime_models(&source())
        .expect("Cabinet + arrival-context pressure source should check")
}

fn open_fresh(checked: &CheckedSource) -> CabinetRuntime {
    PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh Cabinet should open")
}

fn selected_name(runtime: &mut CabinetRuntime) -> String {
    match runtime
        .value("selectedFolderName")
        .expect("selectedFolderName should evaluate")
    {
        Value::String(value) => value,
        other => panic!("selectedFolderName should be String, got {other:?}"),
    }
}

/// Linear control:
///
/// ```text
/// Notes -> Projects -> Elanu
/// route [0, 0]
/// prefix [0]
/// ```
///
/// Replaying the known arrival prefix should select Projects. This uses only the
/// already-established bounded route composition; there is no parent lookup.
#[test]
fn known_arrival_prefix_replay_expresses_one_level_up_before_and_after_restart() {
    let checked = checked();
    let mut runtime = open_fresh(&checked);

    runtime
        .run_action("initialize")
        .expect("initialize should publish");
    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .expect("Projects creation should publish");
    runtime
        .run_action_with_values("createFolder", &[Value::String("Elanu".into())])
        .expect("Elanu creation should publish");
    runtime
        .run_action("selectHome")
        .expect("home selection should publish");

    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("full resident arrival route should select Elanu");

    assert_eq!(selected_name(&mut runtime), "Elanu");

    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(1)],
        )
        .expect("resident arrival-prefix replay should select Projects");

    assert_eq!(selected_name(&mut runtime), "Projects");

    // Return to Home before persistence/restart so the restarted half begins
    // from a neutral committed selection.
    runtime
        .run_action("selectHome")
        .expect("home selection should publish");

    let provider = runtime.into_provider();

    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("Cabinet should reopen");

    assert!(
        restarted.dormant_backing_keys().len() >= 4,
        "dynamic folders should reopen dormant"
    );

    // Establish the same arrival after restart.
    restarted
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("full route should cross dormant intermediates after restart");

    // Now replay the known prefix. The host supplies only its transient route
    // gesture; it does not provide identity or parentage.
    restarted
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(1)],
        )
        .expect("arrival-prefix replay should cross dormant state after restart");

    restarted
        .materialize_designation("selectedFolder")
        .expect("final selected folder should materialize for observation");

    assert_eq!(selected_name(&mut restarted), "Projects");
}

/// Ambiguity pressure:
///
/// The same exact Shared folder occurs under both Left and Right. Shared's
/// lifetime/rooting owner is Left, but structural arrival may be:
///
/// ```text
/// [0, 0] -> Left  -> Shared
/// [1, 0] -> Right -> Shared
/// ```
///
/// Prefix replay must therefore follow the supplied arrival context:
///
/// ```text
/// prefix [0] -> Left
/// prefix [1] -> Right
/// ```
///
/// If "Up" were inferred from child identity or lifetime provenance, these two
/// arrivals could not produce different structural contexts.
#[test]
fn same_exact_identity_can_have_distinct_structural_up_contexts_by_arrival() {
    let checked = checked();
    let mut runtime = open_fresh(&checked);

    runtime
        .run_action("initialize")
        .expect("initialize should publish");
    runtime
        .run_action("seedAmbiguousArrival")
        .expect("ambiguous structural fixture should publish");

    // Arrive at Shared through Left.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("Left -> Shared route should succeed");

    assert_eq!(selected_name(&mut runtime), "Shared");

    // Mutate through that selection so the alternate route can prove it reaches
    // the same modeled identity rather than an independently named lookalike.
    runtime
        .run_action_with_values(
            "renameSelectedFolder",
            &[Value::String("Shared exact identity".into())],
        )
        .expect("renaming Shared should publish");

    // The known prefix for the Left arrival means Up -> Left.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(1)],
        )
        .expect("Left arrival prefix should succeed");

    assert_eq!(selected_name(&mut runtime), "Left");

    // Arrive at the very same Shared identity through Right.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(1), Value::Int(0), Value::Int(2)],
        )
        .expect("Right -> Shared route should succeed");

    assert_eq!(
        selected_name(&mut runtime),
        "Shared exact identity",
        "the Right path must reach the same modeled identity renamed through Left"
    );

    // Shared is rooted under Left, but this arrival came through Right.
    // Its arrival-prefix Up must therefore be Right, not its rooting owner.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(1), Value::Int(0), Value::Int(1)],
        )
        .expect("Right arrival prefix should succeed");

    assert_eq!(
        selected_name(&mut runtime),
        "Right",
        "structural Up from a known arrival context must not collapse to provenance"
    );

    // And the original Left path still reaches the same exact identity.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("Left -> Shared should remain reachable");

    assert_eq!(selected_name(&mut runtime), "Shared exact identity");

    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(1)],
        )
        .expect("Left arrival prefix should remain valid");

    assert_eq!(selected_name(&mut runtime), "Left");
}

/// Staleness pressure: an index route is evaluated against current structure.
///
/// Initial structure:
///
/// ```text
/// Notes
/// ├── Left
/// │   └── Shared
/// └── Right
///     └── Shared
/// ```
///
/// Arrival `[0, 0]` means Left -> Shared. After removing Left's occurrence from
/// Home, Home's current index 0 is Right. Replaying the old `[0]` prefix must
/// therefore resolve to Right.
///
/// This is deliberately *not* stable arrival identity. It demonstrates that a
/// route is a transient structural gesture interpreted against current state.
#[test]
fn stale_arrival_prefix_can_retarget_after_structure_changes() {
    let checked = checked();
    let mut runtime = open_fresh(&checked);

    runtime
        .run_action("initialize")
        .expect("initialize should publish");
    runtime
        .run_action("seedAmbiguousArrival")
        .expect("ambiguous structural fixture should publish");

    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("initial Left -> Shared arrival should succeed");

    assert_eq!(selected_name(&mut runtime), "Shared");

    runtime
        .run_action("removeLeftFromHome")
        .expect("removing Left's Home occurrence should publish");

    // Replay the prefix from the *old* arrival. Index 0 now denotes Right.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(1)],
        )
        .expect("old prefix should still be a valid current structural gesture");

    assert_eq!(
        selected_name(&mut runtime),
        "Right",
        "the stale index prefix must resolve against current structure, not preserve old arrival"
    );

    // More sharply: the old full route still reaches Shared, but now through
    // Right rather than through the occurrence by which the user originally arrived.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("old full route should now mean Right -> Shared");

    assert_eq!(selected_name(&mut runtime), "Shared");
}

/// Staleness pressure: a route may instead become invalid.
///
/// Removing Shared's Left occurrence leaves Left present but with no child at
/// index 0. Replaying the original `[0, 0]` route must fail rather than recover
/// Shared through some other occurrence, provenance edge, or reverse lookup.
///
/// The failed replay must remain one atomic transition: `selectHome()` and the
/// first segment cannot commit independently.
#[test]
fn stale_arrival_route_can_fail_when_the_occurrence_disappears() {
    let checked = checked();
    let mut runtime = open_fresh(&checked);

    runtime
        .run_action("initialize")
        .expect("initialize should publish");
    runtime
        .run_action("seedAmbiguousArrival")
        .expect("ambiguous structural fixture should publish");

    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("initial Left -> Shared arrival should succeed");

    assert_eq!(selected_name(&mut runtime), "Shared");

    runtime
        .run_action("removeSharedFromLeft")
        .expect("removing Shared's Left occurrence should publish");

    // Give rollback something observable to preserve.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(1), Value::Int(0), Value::Int(2)],
        )
        .expect("Right -> Shared should remain valid");

    assert_eq!(selected_name(&mut runtime), "Shared");

    let error = runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect_err("old Left -> Shared route should now be invalid");

    assert!(
        error.message.contains("out of bounds"),
        "unexpected stale-route failure: {}",
        error.message
    );

    assert_eq!(
        selected_name(&mut runtime),
        "Shared",
        "failed stale-route replay must roll back the staged Home/Left selection"
    );

    // The exact same Shared identity is still structurally reachable through Right.
    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(1), Value::Int(0), Value::Int(2)],
        )
        .expect("alternate structural occurrence should remain independently reachable");

    assert_eq!(selected_name(&mut runtime), "Shared");
}

/// Cross-surface pressure:
///
/// Two different structural arrival histories can end in the exact same committed
/// Elanu application world:
///
/// ```text
/// history A: Notes -> Left  -> Shared
/// history B: Notes -> Right -> Shared
/// ```
///
/// `Shared` is the same exact modeled identity in both histories. If the durable
/// application state is identical afterward, then a second presentation surface
/// observing only shared Elanu state cannot determine which structural occurrence
/// was used to arrive there.
///
/// Yet arrival-relative Up has different correct answers:
///
/// ```text
/// history A -> Up means Left
/// history B -> Up means Right
/// ```
///
/// This does not prove arrival context belongs in the language. It proves only
/// that it is absent from the current shared application state.
#[test]
fn identical_shared_application_state_can_require_different_arrival_relative_up() {
    let checked = checked();

    let mut seeded = open_fresh(&checked);
    seeded
        .run_action("initialize")
        .expect("initialize should publish");
    seeded
        .run_action("seedAmbiguousArrival")
        .expect("ambiguous structural fixture should publish");

    let base_provider = seeded.into_provider();

    // Counterfactual presentation history A:
    // arrive at the exact Shared identity through Left.
    let mut arrived_through_left =
        PartialPersistentRuntime::open(checked.clone(), base_provider.clone())
            .expect("Left-history runtime should reopen");

    arrived_through_left
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("Left -> Shared arrival should succeed");

    // Counterfactual presentation history B:
    // arrive at the same exact Shared identity through Right.
    let mut arrived_through_right = PartialPersistentRuntime::open(checked.clone(), base_provider)
        .expect("Right-history runtime should reopen");

    arrived_through_right
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(1), Value::Int(0), Value::Int(2)],
        )
        .expect("Right -> Shared arrival should succeed");

    let left_world = arrived_through_left.into_provider();
    let right_world = arrived_through_right.into_provider();

    // The two histories must leave exactly the same durable application truth.
    // Ignore provider load logs: they describe implementation work, not committed
    // Elanu state.
    assert_eq!(
        left_world.manifest, right_world.manifest,
        "arrival occurrence must not be smuggled into the committed manifest"
    );
    assert_eq!(
        left_world.backing, right_world.backing,
        "arrival occurrence must not be smuggled into modeled backing"
    );

    // Reopen both indistinguishable application worlds.
    let mut left_history = PartialPersistentRuntime::open(checked.clone(), left_world)
        .expect("Left-history world should reopen");
    let mut right_history = PartialPersistentRuntime::open(checked, right_world)
        .expect("Right-history world should reopen");

    // Both surfaces observing shared Elanu state see the same selected identity.
    left_history
        .materialize_designation("selectedFolder")
        .expect("Left-history selection should materialize");
    right_history
        .materialize_designation("selectedFolder")
        .expect("Right-history selection should materialize");

    assert_eq!(selected_name(&mut left_history), "Shared");
    assert_eq!(selected_name(&mut right_history), "Shared");

    // But if the presentation also supplies the arrival context it witnessed,
    // arrival-relative Up has different answers.
    left_history
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(1)],
        )
        .expect("Left arrival prefix should replay");

    right_history
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(1), Value::Int(0), Value::Int(1)],
        )
        .expect("Right arrival prefix should replay");

    left_history
        .materialize_designation("selectedFolder")
        .expect("Left Up result should materialize");
    right_history
        .materialize_designation("selectedFolder")
        .expect("Right Up result should materialize");

    assert_eq!(
        selected_name(&mut left_history),
        "Left",
        "arrival through Left requires Left as arrival-relative Up"
    );
    assert_eq!(
        selected_name(&mut right_history),
        "Right",
        "arrival through Right requires Right as arrival-relative Up"
    );
}
