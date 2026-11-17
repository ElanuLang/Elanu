//! Pressure: can a shared, conservative arrival-context coordinator support
//! structural Up without making hosts reconstruct Cabinet application truth?
//!
//! This is a test-only B/D host-adapter candidate, not Elanu source semantics.
//! An explicit root and route are presentation gestures, not modeled identity.
//!
//! Prior PR #85 demonstrated that per-surface histories diverge when one
//! source-owned selectedFolder is shared. This probe uses ONE transient context
//! for both surfaces and invalidates it after any other successful transition.
//!
//! A deliberately conservative invalidation is safe but may be unusable after
//! selection-preserving edits. That cost is tested, not silently accepted.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const CABINET: &str = include_str!("../../examples/cabinet.elnu");

// Route replay is a real Cabinet-shaped source action using PR #84's established
// single ordinary sequence argument composition. It is added only to this
// pressure fixture, not to production Cabinet source.
const ROUTE_SOURCE: &str = r#"
action selectHomeRoute(route: [Int]) {
    selectHome()
    selectFolder(route)
}

action duplicateFirstHomeChild {
    selectHome()
    with selectedFolder.folders[0] as duplicate {
        insert duplicate into selectedFolder.folders
    }
}
"#;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrivalRoot {
    Home,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Arrival {
    root: ArrivalRoot,
    segments: Vec<i64>,
}

// The adapter owns presentation context only. Runtime still owns all selected
// designation identity, membership, transactionality, and partial residency.
//
// Unknown actions are treated uniformly: on success they invalidate context.
// No action-name allowlist, selected-model identity, parent lookup, structural
// graph, or source-specific dependency knowledge appears here.
struct SharedArrivalSession {
    runtime: PartialPersistentRuntime<MemoryProvider>,
    arrival: Option<Arrival>,
}

impl SharedArrivalSession {
    fn new() -> Self {
        let source = format!("{CABINET}\n{ROUTE_SOURCE}");
        let checked = check_source_with_runtime_models(&source)
            .expect("Cabinet with route pressure source should check");

        let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
            .expect("Cabinet runtime should open");
        runtime
            .run_action("initialize")
            .expect("Cabinet should initialize");

        Self {
            runtime,
            // Initialization selected Home in source, but this adapter has not
            // observed any explicit presentation arrival gesture yet.
            arrival: None,
        }
    }

    fn replay(&mut self, route: &[i64]) -> Result<(), RuntimeError> {
        self.runtime.run_action_with_values(
            "selectHomeRoute",
            &[Value::ValueSequence(
                route.iter().copied().map(Value::Int).collect(),
            )],
        )
    }

    fn home(&mut self) -> Result<(), RuntimeError> {
        self.replay(&[])?;
        self.arrival = Some(Arrival {
            root: ArrivalRoot::Home,
            segments: Vec::new(),
        });
        Ok(())
    }

    fn enter(&mut self, index: i64) -> Result<(), RuntimeError> {
        let mut next = self
            .arrival
            .clone()
            .ok_or_else(|| RuntimeError::new("arrival context unavailable"))?;

        next.segments.push(index);
        self.replay(&next.segments)?;
        self.arrival = Some(next);
        Ok(())
    }

    fn up(&mut self) -> Result<(), RuntimeError> {
        let mut next = self
            .arrival
            .clone()
            .ok_or_else(|| RuntimeError::new("arrival context unavailable"))?;

        if next.segments.pop().is_none() {
            return Err(RuntimeError::new("already at arrival root"));
        }

        self.replay(&next.segments)?;
        self.arrival = Some(next);
        Ok(())
    }

    fn ordinary_action(&mut self, name: &str, values: &[Value]) -> Result<(), RuntimeError> {
        self.runtime.run_action_with_values(name, values)?;
        // This does not depend on knowing whether 'name' can change selection,
        // membership, or route validity. Unknown successful actions all
        // invalidate. Failed actions roll back and retain the prior route.
        self.arrival = None;
        Ok(())
    }

    fn selected_name(&mut self) -> String {
        match self
            .runtime
            .value("selectedFolderName")
            .expect("selected name")
        {
            Value::String(name) => name,
            other => panic!("selectedFolderName must be a String, got {other}"),
        }
    }

    fn route(&self) -> Option<&[i64]> {
        self.arrival
            .as_ref()
            .map(|arrival| arrival.segments.as_slice())
    }
}

fn create_projects_archive(session: &mut SharedArrivalSession) {
    session
        .ordinary_action("createFolder", &[Value::String("Projects".into())])
        .expect("Projects creation");
    session
        .ordinary_action("createFolder", &[Value::String("Archive".into())])
        .expect("Archive creation");
    session.home().expect("explicit Home gesture");
}

#[test]
fn shared_context_composes_across_two_surfaces_without_identity_leakage() {
    let mut session = SharedArrivalSession::new();
    create_projects_archive(&mut session);

    // The two surface handles share one coordinator and one native runtime,
    // just as the Cabinet terminal and local browser currently share runtime.
    let shared = Arc::new(Mutex::new(session));
    let terminal = Arc::clone(&shared);
    let browser = Arc::clone(&shared);

    {
        let mut terminal = terminal.lock().unwrap();
        terminal.enter(0).expect("terminal enters Projects");
        assert_eq!(terminal.selected_name(), "Projects");
        assert_eq!(terminal.route(), Some(&[0][..]));
    }

    {
        let mut browser = browser.lock().unwrap();
        browser.enter(0).expect("browser continues into Archive");
        assert_eq!(browser.selected_name(), "Archive");
        assert_eq!(browser.route(), Some(&[0, 0][..]));
    }

    {
        let mut terminal = terminal.lock().unwrap();
        terminal.up().expect("terminal goes Up to Projects");
        assert_eq!(terminal.selected_name(), "Projects");
        assert_eq!(terminal.route(), Some(&[0][..]));
    }

    {
        let mut browser = browser.lock().unwrap();
        browser.up().expect("browser goes Up to Home");
        assert_eq!(browser.selected_name(), "Notes");
        assert_eq!(browser.route(), Some(&[][..]));
    }
}

#[test]
fn unknown_source_reselection_invalidates_without_host_action_knowledge() {
    let mut session = SharedArrivalSession::new();
    create_projects_archive(&mut session);
    session.enter(0).expect("enter Projects");
    assert_eq!(session.selected_name(), "Projects");
    assert_eq!(session.route(), Some(&[0][..]));

    // createFolder is source-owned and changes selectedFolder without an index
    // gesture. The adapter does not attempt to infer the new child position.
    session
        .ordinary_action("createFolder", &[Value::String("Drafts".into())])
        .expect("create and select Drafts");
    assert_eq!(session.selected_name(), "Drafts");
    assert_eq!(session.route(), None);

    let error = session.up().expect_err("Up requires known arrival context");
    assert!(error.message.contains("arrival context unavailable"));
    assert_eq!(session.selected_name(), "Drafts");

    session.home().expect("explicit Home reestablishes context");
    assert_eq!(session.selected_name(), "Notes");
    assert_eq!(session.route(), Some(&[][..]));
}

#[test]
fn conservative_invalidation_loses_up_after_selection_preserving_rename() {
    let mut session = SharedArrivalSession::new();
    create_projects_archive(&mut session);
    session.enter(0).expect("enter Projects");

    // This action changes only the selected Folder's name, not its identity,
    // structural position, or membership. Generic blanket invalidation still
    // loses the valid route. That is a UX cost, not a semantic necessity.
    session
        .ordinary_action(
            "renameSelectedFolder",
            &[Value::String("Projects renamed".into())],
        )
        .expect("rename succeeds");

    assert_eq!(session.selected_name(), "Projects renamed");
    assert_eq!(session.route(), None);
    assert!(session
        .up()
        .expect_err("safe conservative adapter has no Up after rename")
        .message
        .contains("arrival context unavailable"));
}

#[test]
fn failed_unknown_action_preserves_context_and_allows_up() {
    let mut session = SharedArrivalSession::new();
    create_projects_archive(&mut session);
    session.enter(0).expect("enter Projects");

    let error = session
        .ordinary_action("selectFolder", &[Value::Int(99)])
        .expect_err("invalid selected child must fail atomically");

    assert!(error.message.contains("out of bounds"));
    assert_eq!(session.selected_name(), "Projects");
    assert_eq!(session.route(), Some(&[0][..]));

    session.up().expect("context remains valid after rollback");
    assert_eq!(session.selected_name(), "Notes");
}

#[test]
fn failed_route_step_preserves_prior_arrival_context() {
    let mut session = SharedArrivalSession::new();
    create_projects_archive(&mut session);
    session.enter(0).expect("enter Projects");

    let error = session.enter(99).expect_err("invalid route fails");
    assert!(error.message.contains("out of bounds"));
    assert_eq!(session.selected_name(), "Projects");
    assert_eq!(session.route(), Some(&[0][..]));

    session.up().expect("Up still resolves from Projects");
    assert_eq!(session.selected_name(), "Notes");
}

#[test]
fn duplicate_membership_does_not_collapse_distinct_arrival_routes() {
    let mut session = SharedArrivalSession::new();
    session
        .ordinary_action("createFolder", &[Value::String("Projects".into())])
        .expect("create Projects");
    session
        .ordinary_action("duplicateFirstHomeChild", &[])
        .expect("one exact child should occur twice under Home");

    session.home().expect("explicit Home gesture");
    assert_eq!(
        session
            .runtime
            .designation_member_len("homeFolder", "folders")
            .expect("Home folder count"),
        2
    );

    session.enter(0).expect("first occurrence navigation");
    assert_eq!(session.selected_name(), "Projects");
    assert_eq!(session.route(), Some(&[0][..]));

    session.home().expect("re-establish Home");
    session.enter(1).expect("second occurrence navigation");
    assert_eq!(session.selected_name(), "Projects");
    assert_eq!(session.route(), Some(&[1][..]));

    // The same child identity was inserted twice by the source action, but
    // arrival histories remain distinct transient index gestures. No identity
    // query, reverse lookup, or stable occurrence identity is involved.
}
