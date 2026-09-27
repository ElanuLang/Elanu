//! Pressure experiment: runtime-length route replay.
//!
//! Arrival-context pressure established that a presentation may legitimately own
//! a transient structural route. The remaining question is whether a host can
//! replay an arbitrary number of route segments as one Elanu transition using
//! the current surface.
//!
//! This file adds no mechanism. It records two negative facts:
//!
//! 1. repeated top-level host action calls are separate transactions;
//! 2. the current source surface does not accept a scalar `[Int]` route parameter.
//!
//! Neither result selects a solution.

use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models, parse_source,
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

/// Probe the most obvious source representation for presentation-owned route
/// indices.
///
/// Current ordered sequences are `[live T]` structural designation sequences.
/// This test records that `[Int]` is not already an accepted general sequence
/// parameter surface.
///
/// If this unexpectedly starts parsing in the future, the pressure must be
/// reconsidered rather than silently preserving this negative assertion.
#[test]
fn scalar_route_sequence_is_not_already_an_action_parameter_surface() {
    let source = r#"
action replay(route: [Int]) {
}
"#;

    assert!(
        parse_source(source).is_err(),
        "the current language unexpectedly accepts `[Int]` action parameters"
    );
}
