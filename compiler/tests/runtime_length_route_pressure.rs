//! Pressure experiment: runtime-length route replay.
//!
//! Arrival-context pressure established that a presentation may legitimately own
//! a transient structural route. The remaining question is whether a host can
//! replay an arbitrary number of route segments as one Elanu transition using
//! the current surface.
//!
//! This file adds no route-consumption mechanism. It records the remaining
//! composition pressure:
//!
//! 1. repeated top-level host action calls are separate transactions;
//! 2. `[Int]` now crosses the partial-persistent host action boundary as an
//!    already-structured runtime value.
//!
//! These facts still do not select a route-consumption mechanism.

use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const CABINET: &str = include_str!("../../examples/cabinet.elnu");

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

type CabinetRuntime = PartialPersistentRuntime<MemoryProvider>;

fn runtime() -> CabinetRuntime {
    let checked =
        check_source_with_runtime_models(CABINET).expect("Cabinet source should continue to check");

    PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("Cabinet partial runtime should initialize")
}

/// A host cannot implement an arbitrary route by simply calling `selectFolder`
/// once per segment.
///
/// Each public top-level action commits independently. Therefore failure of a
/// later segment cannot roll back an earlier successful segment.
///
/// This is exactly the behavior an atomic route replay must *not* have.
#[test]
fn host_segment_loop_cannot_supply_atomic_route_semantics() {
    let mut runtime = runtime();

    runtime
        .run_action("initialize")
        .expect("initialize should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .expect("Projects creation should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Elanu".into())])
        .expect("Elanu creation should commit");

    runtime
        .run_action("selectHome")
        .expect("Home selection should commit");

    // Imagine the host replaying route [0, 9].
    //
    // Segment 0 commits Projects immediately.
    runtime
        .run_action_with_values("selectFolder", &[Value::Int(0)])
        .expect("first host-driven segment should commit");

    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Projects".into())
    );

    // The later segment fails...
    let error = runtime
        .run_action_with_values("selectFolder", &[Value::Int(9)])
        .expect_err("second segment should be out of bounds");

    assert!(
        error.message.contains("out of bounds"),
        "unexpected route failure: {}",
        error.message
    );

    // ...but the first segment is already committed.
    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Projects".into()),
        "separate host action calls cannot roll the whole route back to Home"
    );
}

/// Positive control: when the route shape is statically bounded, the current
/// language already supplies the semantic behavior an arbitrary route needs.
///
/// The source explicitly names two route segments, but nested actions provide:
///
/// - ordered application;
/// - transaction-visible selection from the prior segment;
/// - one shared top-level transaction;
/// - rollback of the whole route when a later segment fails.
///
/// Therefore the pressure is not "Elanu cannot compose navigation atomically."
/// It is that current action input cannot carry runtime-sized transient route
/// structure into that already-established composition.
#[test]
fn bounded_source_route_already_has_required_atomic_semantics() {
    let source = format!(
        "{CABINET}\n{}",
        r#"
action replayTwo(first: Int, second: Int) {
    selectHome()
    selectFolder(first)
    selectFolder(second)
}
"#
    );

    let checked =
        check_source_with_runtime_models(&source).expect("bounded two-segment route should check");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("bounded-route runtime should initialize");

    runtime
        .run_action("initialize")
        .expect("initialize should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .expect("Projects creation should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Elanu".into())])
        .expect("Elanu creation should commit");

    runtime
        .run_action("selectHome")
        .expect("Home selection should commit");

    runtime
        .run_action_with_values("replayTwo", &[Value::Int(0), Value::Int(0)])
        .expect("bounded route should select Elanu atomically");

    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Elanu".into())
    );

    runtime
        .run_action("selectHome")
        .expect("Home reset should commit");

    let error = runtime
        .run_action_with_values("replayTwo", &[Value::Int(0), Value::Int(9)])
        .expect_err("invalid second segment should fail the whole route");

    assert!(
        error.message.contains("out of bounds"),
        "unexpected route failure: {}",
        error.message
    );

    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Notes".into()),
        "failure of the later segment must roll the complete bounded route back"
    );
}

/// The existing runtime sequence carrier is not a generic transient collection.
///
/// `Value::Sequence` carries modeled designation identities. The partial-runtime
/// host boundary deliberately refuses to translate such a value into an action
/// argument.
///
/// Ordinary scalar sequences now have their own runtime representation.
/// They must remain distinct from this identity-bearing carrier.
#[test]
fn modeled_identity_sequence_is_not_a_transient_host_value_carrier() {
    let mut runtime = runtime();

    runtime
        .run_action("initialize")
        .expect("initialize should commit");

    let error = runtime
        .run_action_with_values(
            "selectFolder",
            &[Value::Sequence {
                element_model: "Folder".into(),
                targets: Vec::new(),
            }],
        )
        .expect_err("host values must not carry modeled identity sequences");

    assert!(
        error
            .message
            .contains("cannot carry modeled identity sequences"),
        "unexpected host sequence rejection: {}",
        error.message
    );
}

/// Structural sequence syntax exists, but sequence values cannot currently cross
/// an action boundary even when they are `[live T]`.
///
/// This is distinct from `[Int]` not being a recognized sequence type at all.
/// The source surface recognizes `[live Folder]`, then the bootstrap sequence
/// integration deliberately rejects it as an action parameter.
///
/// Therefore runtime-sized route pressure reaches a genuine structured-input
/// boundary, not merely a missing scalar element type.
#[test]
fn structural_sequence_is_not_currently_an_action_parameter_surface() {
    let source = format!(
        "{CABINET}\n{}",
        r#"
action acceptFolders(route: [live Folder]) {
}
"#
    );

    let errors = check_source_with_runtime_models(&source)
        .expect_err("sequence-valued action parameters should remain unsupported");

    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("does not yet support sequence action parameters")
        }),
        "unexpected sequence-parameter diagnostics: {errors:?}"
    );
}

/// Ordinary `[Int]` action parameters check, and both the plain runtime and the
/// partial-persistent host bridge now receive a runtime-sized `ValueSequence`.
///
/// The partial-persistent bridge routes ordinary sequences through the runtime
/// action-value path rather than reconstructing them as source expressions. This
/// proves only that the boundary carries them: nothing here indexes, traverses,
/// mutates, persists, or routes over the sequence, and retry, rollback,
/// dormant-backing, and durable-publication behavior are unchanged.
#[test]
fn partial_persistent_host_bridge_carries_ordinary_sequences() {
    let source = format!(
        "{CABINET}\n{}",
        r#"
action acceptRoute(route: [Int]) {
}
"#
    );

    let checked =
        check_source_with_runtime_models(&source).expect("[Int] action parameter should check");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("partial-persistent runtime should initialize");

    runtime
        .run_action_with_values(
            "acceptRoute",
            &[Value::ValueSequence(vec![
                Value::Int(0),
                Value::Int(2),
                Value::Int(1),
            ])],
        )
        .expect("partial-persistent host bridge should carry an ordinary sequence");
}

/// A modeled identity sequence is a distinct surface and must stay invisible to
/// host action values even on the runtime-value path.
///
/// The argument list here is otherwise valid: `acceptRoutes` declares exactly
/// two `[Int]` parameters and receives two arguments. So this establishes that
/// the rejection is the identity firewall itself, not an arity error that
/// happened to fire first.
#[test]
fn partial_persistent_host_bridge_still_rejects_identity_sequences() {
    let source = format!(
        "{CABINET}\n{}",
        r#"
action acceptRoutes(first: [Int], second: [Int]) {
}
"#
    );

    let checked = check_source_with_runtime_models(&source)
        .expect("two [Int] action parameters should check");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("partial-persistent runtime should initialize");

    let error = runtime
        .run_action_with_values(
            "acceptRoutes",
            &[
                Value::ValueSequence(vec![Value::Int(0)]),
                Value::Sequence {
                    element_model: "Folder".into(),
                    targets: Vec::new(),
                },
            ],
        )
        .expect_err("modeled identity sequences must remain invisible to host action values");

    assert!(
        error
            .message
            .contains("cannot carry modeled identity sequences"),
        "unexpected identity-sequence rejection: {}",
        error.message
    );
}

/// Modeled identity must stay host-invisible even when nested inside an ordinary
/// value sequence.
///
/// `Value::ValueSequence` is recursively structured, so a shallow top-level guard
/// would let a nested identity sequence reach coercion. Today no source type is
/// `[[T]]`, so that nested value would fail an expected-type mismatch rather
/// than the identity firewall. This test pins the structural rejection instead,
/// so the contract holds by construction rather than by incidental typing.
///
/// Using `[[live Folder]]` here would be stronger still, but the language has no
/// nested ordered-sequence syntax to declare it.
#[test]
fn partial_persistent_host_bridge_rejects_nested_identity_sequences() {
    let source = format!(
        "{CABINET}\n{}",
        r#"
action acceptNested(route: [Int]) {
}
"#
    );

    let checked =
        check_source_with_runtime_models(&source).expect("[Int] action parameter should check");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("partial-persistent runtime should initialize");

    let error = runtime
        .run_action_with_values(
            "acceptNested",
            &[Value::ValueSequence(vec![Value::Sequence {
                element_model: "Folder".into(),
                targets: Vec::new(),
            }])],
        )
        .expect_err("modeled identity must remain host-invisible even when nested");

    assert!(
        error
            .message
            .contains("cannot carry modeled identity sequences"),
        "unexpected nested identity rejection: {}",
        error.message
    );
}
/// One ordinary sequence argument can drive ordered repeated action application
/// without exposing traversal machinery.
///
/// The route elements are applied in sequence order inside the enclosing action's
/// existing transaction. A later failure therefore rolls back all earlier route
/// steps together.
#[test]
fn one_sequence_argument_replays_route_atomically() {
    let source = format!(
        "{CABINET}\n{}",
        r#"
action replayRoute(route: [Int]) {
    selectHome()
    selectFolder(route)
}
"#
    );

    let checked = check_source_with_runtime_models(&source)
        .expect("single ordered argument should infer repeated action application");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("runtime should initialize");

    runtime
        .run_action("initialize")
        .expect("initialize should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .expect("Projects creation should commit");

    runtime
        .run_action_with_values("createFolder", &[Value::String("Elanu".into())])
        .expect("Elanu creation should commit");

    runtime
        .run_action_with_values(
            "replayRoute",
            &[Value::ValueSequence(vec![Value::Int(0), Value::Int(0)])],
        )
        .expect("runtime-sized route should replay atomically");

    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Elanu".into())
    );

    runtime
        .run_action("selectHome")
        .expect("Home reset should commit");

    let error = runtime
        .run_action_with_values(
            "replayRoute",
            &[Value::ValueSequence(vec![Value::Int(0), Value::Int(9)])],
        )
        .expect_err("later route failure should roll back the whole replay");

    assert!(
        error.message.contains("out of bounds"),
        "unexpected route failure: {}",
        error.message
    );

    assert_eq!(
        runtime.value("selectedFolderName").unwrap(),
        Value::String("Notes".into()),
        "later route failure must roll the complete runtime-sized replay back"
    );
}
/// Pressure probe: automatic action lifting is not mechanically inferable when
/// more than one ordered argument would need traversal.
///
/// Given two `[Int]` values and an action expecting two `Int` values, source
/// syntax alone does not tell us whether application means zip, Cartesian
/// product, broadcasting, or something else.
///
/// This prevents the single-sequence route case from accidentally selecting a
/// general "sequence arguments automatically lift action calls" rule.
#[test]
fn multiple_sequence_arguments_do_not_select_a_general_lifting_rule() {
    let source = r#"
action consumePair(left: Int, right: Int) {
}

action consumeRoutes(left: [Int], right: [Int]) {
    consumePair(left, right)
}
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("multiple sequence arguments should not implicitly select traversal semantics");

    assert!(
        errors
            .iter()
            .any(|error| { error.message.contains("has type [Int] but expected Int") }),
        "unexpected multiple-sequence diagnostics: {errors:?}"
    );
}
/// Scalar companion arguments are evaluated once at the lifted call site and
/// reused for every ordered application.
///
/// `offset` begins at 10. Each step mutates the state `offset`, but the value
/// supplied to `consumeStep` remains the original captured 10. If the companion
/// expression were re-evaluated per element, `total` would become 21 instead of
/// 20.
#[test]
fn one_sequence_argument_reuses_scalar_companion_value() {
    let source = r#"
state offset = 10
state total = 0

action consumeStep(index: Int, value: Int) {
    total += value
    offset += 1
}

action consumeRoute(route: [Int]) {
    consumeStep(route, offset)
}
"#;

    let checked = check_source_with_runtime_models(source)
        .expect("one sequence argument with a scalar companion should check");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("runtime should initialize");

    runtime
        .run_action_with_values(
            "consumeRoute",
            &[Value::ValueSequence(vec![Value::Int(1), Value::Int(2)])],
        )
        .expect("ordered application with scalar companion should succeed");

    assert_eq!(
        runtime.value("total").unwrap(),
        Value::Int(20),
        "scalar companion should be captured once and reused"
    );

    assert_eq!(
        runtime.value("offset").unwrap(),
        Value::Int(12),
        "each repeated application should still observe and mutate shared transaction state"
    );
}


/// Repeated value application does not infer repeated writable authority.
///
/// A concrete sequence value can drive an ordinary value parameter, but a
/// writable `state` grant is a separate semantic axis. The narrow lifting rule
/// deliberately does not transport or duplicate that authority.
#[test]
fn sequence_lifting_does_not_infer_writable_authority() {
    let source = r#"
state total = 0

action consume(index: Int, state target: Int) {
    target += index
}

action consumeRoute(route: [Int]) {
    consume(route, state total)
}
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("sequence lifting must not infer writable state authority");

    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("sequence-lifted action call cannot include writable state parameters")
        }),
        "unexpected writable-authority diagnostics: {errors:?}"
    );
}

/// Empty ordered input performs zero repeated applications.
///
/// This follows directly from ordered repeated application rather than adding
/// special empty-sequence behavior.
#[test]
fn empty_sequence_performs_zero_lifted_applications() {
    let source = r#"
state calls = 0

action consume(index: Int) {
    calls += 1
}

action consumeRoute(route: [Int]) {
    consume(route)
}
"#;

    let checked =
        check_source_with_runtime_models(source).expect("single sequence lift should check");

    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("runtime should initialize");

    runtime
        .run_action_with_values("consumeRoute", &[Value::ValueSequence(Vec::new())])
        .expect("empty ordered input should be a successful zero-application replay");

    assert_eq!(
        runtime.value("calls").unwrap(),
        Value::Int(0),
        "empty ordered input must not invoke the lifted action"
    );
}
