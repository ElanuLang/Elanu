//! Pressure experiment: bounded nested navigation ("route") across partial
//! residency.
//!
//! The route is expressed with existing Cabinet surface only:
//!
//! ```text
//! selectHome()                  // Notes
//! selectFolder(first)           // -> Home.folders[first]
//! selectFolder(second)          // -> <staged child>.folders[second]
//! ```
//!
//! Nested action calls share one transaction, so the second segment must see the
//! selection staged by the first. This pressure experiment verifies that the same
//! source behavior remains valid in a fully resident world and after restart when
//! intermediate folder payloads are dormant.

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

/// Route composition added *outside* the example so the shipped Cabinet source
/// stays untouched. No new semantics: nested calls, shared transaction,
/// ordinary indexing.
const ROUTE: &str = r#"
state observedIntermediate = ""
state observedIndexedChildName = ""

action selectRoute(first: Int, second: Int, third: Int, depth: Int) {
    selectHome()

    if depth == 1 {
        selectFolder(first)
    }

    if depth == 2 {
        selectFolder(first)
        selectFolder(second)
    }

    if depth == 3 {
        selectFolder(first)
        selectFolder(second)
        selectFolder(third)
    }
}

action selectRouteRecordingIntermediate(first: Int, second: Int) {
    selectHome()
    selectFolder(first)
    observedIntermediate = selectedFolderName
    selectFolder(second)
}

action readIndexedChildName(index: Int) {
    observedIndexedChildName = selectedFolder.folders[index].name
}
"#;

fn source() -> String {
    format!("{CABINET}\n{ROUTE}")
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
    check_source_with_runtime_models(&source()).expect("cabinet + route source should check")
}

/// Notes
/// └── Projects
///     └── Elanu
///
/// `selectedFolder` is back on Notes at the end of seeding.
fn seeded() -> (CheckedSource, CabinetRuntime) {
    let checked = checked();
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh cabinet should open");

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

    (checked, runtime)
}

fn selected_name(runtime: &mut CabinetRuntime) -> Value {
    runtime
        .value("selectedFolderName")
        .expect("selectedFolderName should evaluate")
}

/// Resident control: the whole route is one commit and each segment sees the
/// selection staged by the previous segment.
#[test]
fn resident_bounded_route_composes_segments_in_one_commit() {
    let (_checked, mut runtime) = seeded();
    assert_eq!(selected_name(&mut runtime), Value::String("Notes".into()));

    runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("resident route should navigate Notes -> Projects -> Elanu");

    assert_eq!(selected_name(&mut runtime), Value::String("Elanu".into()));
    assert_eq!(
        runtime
            .designation_member_len("selectedFolder", "folders")
            .expect("Elanu folders should be observable"),
        0
    );

    let provider = runtime.into_provider();
    assert_eq!(
        provider.commits, 5,
        "seed commits plus exactly one route commit"
    );
}

/// The second segment must consume the first segment's staged selection, not the
/// pre-action selection.
#[test]
fn resident_route_second_segment_observes_staged_first_segment() {
    let (_checked, mut runtime) = seeded();

    runtime
        .run_action_with_values(
            "selectRouteRecordingIntermediate",
            &[Value::Int(0), Value::Int(0)],
        )
        .expect("two-segment route should commit");

    assert_eq!(
        runtime.value("observedIntermediate").unwrap(),
        Value::String("Projects".into()),
        "the first segment's staged selection must be visible to the second"
    );
    assert_eq!(selected_name(&mut runtime), Value::String("Elanu".into()));
}

/// Failure of a later segment must roll the earlier navigation back.
#[test]
fn resident_route_invalid_later_segment_rolls_back_earlier_navigation() {
    let (_checked, mut runtime) = seeded();

    let error = runtime
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(9), Value::Int(0), Value::Int(2)],
        )
        .expect_err("out-of-bounds second segment should fail the route");
    println!("resident out-of-bounds route error: {}", error.message);

    assert_eq!(
        selected_name(&mut runtime),
        Value::String("Notes".into()),
        "failed route must roll back to the pre-action selection"
    );

    let provider = runtime.into_provider();
    assert_eq!(
        provider.commits, 4,
        "failed route must not publish a commit"
    );
    assert_eq!(
        provider.loads.len(),
        0,
        "resident route must not read dormant backing"
    );
}

/// Regression: `Notes -> Projects -> Elanu` after a restart, with every dynamic
/// folder dormant.
///
/// Before the bounded dormant-member-read request this failed at the first
/// structural member read:
///
/// ```text
/// unknown value '__elanu_sm$__elanu_dynamic$Folder$0$folders'
/// ```
///
/// The route now reads exactly the two dormant folders it discovers -- the
/// starting owner and the transaction-staged intermediate -- and publishes one
/// commit.
#[test]
fn restarted_route_navigates_across_dormant_intermediate() {
    let (checked, initial) = seeded();
    let provider = initial.into_provider();
    assert_eq!(provider.commits, 4, "seeding publishes four commits");
    assert_eq!(provider.loads.len(), 0, "seeding reads no backing");

    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("restart should open");
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        4,
        "Notes, Trash, Projects and Elanu are all dormant after restart"
    );

    restarted
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("the bounded route must navigate across the dormant intermediate");

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "only the starting owner and the discovered intermediate may be read"
    );
    assert_ne!(
        provider.loads[0], provider.loads[1],
        "the two segments must read two distinct modeled identities"
    );
    assert_eq!(provider.commits, 5, "the whole route is still one commit");
}

/// Borrowing is transaction-local: a read-only route must not change residency.
#[test]
fn restarted_route_borrows_without_changing_residency() {
    let (checked, initial) = seeded();
    let provider = initial.into_provider();
    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("restart should open");
    assert_eq!(restarted.dormant_backing_keys().len(), 4);

    restarted
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(0), Value::Int(0), Value::Int(2)],
        )
        .expect("route should commit");

    assert_eq!(
        restarted.dormant_backing_keys().len(),
        4,
        "a read-only route must leave every dynamic folder dormant"
    );

    // The route's result is observed through the existing designation bridge,
    // because host observation outside an action has no transaction to borrow in.
    restarted
        .materialize_designation("selectedFolder")
        .expect("the currently selected folder should materialize by designation");
    assert_eq!(
        restarted.value("selectedFolderName").unwrap(),
        Value::String("Elanu".into()),
        "the route must land on the nested folder"
    );
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        3,
        "explicit observation materializes exactly the selected folder"
    );
}

/// Indexed child-member reads have the same residency independence as direct
/// designation-relative member reads. The starting owner is resident, while the
/// child selected by index remains dormant until the action needs its stored member.
#[test]
fn restarted_indexed_member_read_borrows_child_without_promoting_residency() {
    let (checked, initial) = seeded();
    let provider = initial.into_provider();
    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("restart should open");
    assert_eq!(restarted.dormant_backing_keys().len(), 4);

    restarted
        .materialize_designation("selectedFolder")
        .expect("starting folder should materialize for the indexed-member control");
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        3,
        "only the starting folder should be resident before the action"
    );

    restarted
        .run_action_with_values("readIndexedChildName", &[Value::Int(0)])
        .expect("indexed child-member read should borrow the dormant child");

    assert_eq!(
        restarted.value("observedIndexedChildName").unwrap(),
        Value::String("Projects".into())
    );
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        3,
        "reading the indexed child must not promote its residency"
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "one explicit owner load plus one transaction-local child load"
    );
    assert_ne!(
        provider.loads[0], provider.loads[1],
        "the explicit owner and indexed child must be distinct identities"
    );
    assert_eq!(
        provider.commits, 5,
        "the indexed-member read action should publish exactly one candidate"
    );
}
/// The route is still one atomic transition across dormancy.
#[test]
fn restarted_route_invalid_later_segment_rolls_back_whole_route() {
    let (checked, initial) = seeded();
    let provider = initial.into_provider();
    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("restart should open");
    assert_eq!(restarted.dormant_backing_keys().len(), 4);

    let error = restarted
        .run_action_with_values(
            "selectRoute",
            &[Value::Int(0), Value::Int(9), Value::Int(0), Value::Int(2)],
        )
        .expect_err("out-of-bounds second segment should fail the route");
    assert!(
        error.message.contains("out of bounds"),
        "unexpected failure: {}",
        error.message
    );

    assert_eq!(
        restarted.dormant_backing_keys().len(),
        4,
        "a failed route must not publish or retain borrowed residency"
    );
    restarted
        .materialize_designation("selectedFolder")
        .expect("rolled-back selection should still materialize");
    assert_eq!(
        restarted.value("selectedFolderName").unwrap(),
        Value::String("Notes".into()),
        "the whole route must roll back to the pre-action selection"
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.commits, 4,
        "a failed route must not publish a commit"
    );
}

/// The bounded request is transaction-scoped. Host observation outside an action
/// still goes through the existing explicit designation bridge; the runtime does
/// not silently materialize on read.
#[test]
fn host_observation_outside_an_action_still_requires_materialization() {
    let (checked, initial) = seeded();
    let provider = initial.into_provider();
    let mut restarted =
        PartialPersistentRuntime::open(checked, provider).expect("restart should open");

    let cold = restarted.value("selectedFolderName");
    assert!(
        cold.is_err(),
        "no transaction exists to honor a dormant read request"
    );

    restarted
        .materialize_designation("selectedFolder")
        .expect("explicit observation should materialize the designated folder");
    assert_eq!(
        restarted.value("selectedFolderName").unwrap(),
        Value::String("Notes".into())
    );
}
