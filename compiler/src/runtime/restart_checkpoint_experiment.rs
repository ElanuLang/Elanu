use super::*;
use crate::check_source_with_runtime_models;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckpointDynamicModel {
    model_name: String,
    owner: String,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeCheckpoint {
    state_values: HashMap<String, Value>,
    dynamic_models: HashMap<String, CheckpointDynamicModel>,
    next_dynamic_identity: u64,
}

impl Runtime {
    fn capture_restart_checkpoint(&self) -> Result<RuntimeCheckpoint, RuntimeError> {
        if self.transaction.is_some() {
            return Err(RuntimeError::new(
                "restart checkpoint requires a committed runtime with no active transaction",
            ));
        }

        if self.dynamic_model_owners.len() != self.dynamic_model_types.len() {
            return Err(RuntimeError::new(
                "dynamic modeled identity metadata is incomplete",
            ));
        }

        let mut dynamic_models = HashMap::new();
        for (identity, model_name) in &self.dynamic_model_types {
            let owner = self.dynamic_model_owners.get(identity).ok_or_else(|| {
                RuntimeError::new(format!(
                    "dynamic modeled identity '{identity}' is missing owner provenance"
                ))
            })?;
            dynamic_models.insert(
                identity.clone(),
                CheckpointDynamicModel {
                    model_name: model_name.clone(),
                    owner: owner.clone(),
                },
            );
        }

        Ok(RuntimeCheckpoint {
            state_values: self
                .states
                .iter()
                .map(|(name, cell)| (name.clone(), cell.value.clone()))
                .collect(),
            dynamic_models,
            next_dynamic_identity: self.next_dynamic_identity,
        })
    }

    fn restore_restart_checkpoint(
        &mut self,
        checkpoint: &RuntimeCheckpoint,
    ) -> Result<(), RuntimeError> {
        if self.transaction.is_some() {
            return Err(RuntimeError::new(
                "restart checkpoint cannot restore into an active transaction",
            ));
        }
        if !self.dynamic_model_owners.is_empty() || !self.dynamic_model_types.is_empty() {
            return Err(RuntimeError::new(
                "restart checkpoint currently restores only into a fresh runtime",
            ));
        }

        let mut consumed_states = HashSet::new();
        let static_state_names = self.states.keys().cloned().collect::<Vec<_>>();
        for name in static_state_names {
            let value = checkpoint.state_values.get(&name).cloned().ok_or_else(|| {
                RuntimeError::new(format!(
                    "restart checkpoint is missing committed state '{name}'"
                ))
            })?;
            let state = self
                .states
                .get_mut(&name)
                .expect("fresh runtime state should still exist");
            state.value = coerce_value(value, &state.value_type)?;
            state.dependents.clear();
            consumed_states.insert(name);
        }

        for (identity, dynamic) in &checkpoint.dynamic_models {
            if !self
                .runtime_model_templates
                .contains_key(&dynamic.model_name)
            {
                return Err(RuntimeError::new(format!(
                    "restart checkpoint references unknown state model '{}'",
                    dynamic.model_name
                )));
            }
            let owner_exists = self.runtime_model_roots.contains_key(&dynamic.owner)
                || checkpoint.dynamic_models.contains_key(&dynamic.owner);
            if !owner_exists {
                return Err(RuntimeError::new(format!(
                    "restart checkpoint owner '{}' for '{}' is not part of the checked application world",
                    dynamic.owner, identity
                )));
            }
        }

        self.dynamic_model_owners = checkpoint
            .dynamic_models
            .iter()
            .map(|(identity, dynamic)| (identity.clone(), dynamic.owner.clone()))
            .collect();
        self.dynamic_model_types = checkpoint
            .dynamic_models
            .iter()
            .map(|(identity, dynamic)| (identity.clone(), dynamic.model_name.clone()))
            .collect();

        for (identity, dynamic) in &checkpoint.dynamic_models {
            let template = self
                .runtime_model_templates
                .get(&dynamic.model_name)
                .cloned()
                .expect("checkpoint model existence was validated");
            let context = ModelRuntimeContext {
                root: identity.clone(),
                members: template
                    .members
                    .iter()
                    .map(|member| member.name.clone())
                    .collect(),
            };

            for member in template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::Derived)
            {
                let name = model_binding_name(identity, &member.name);
                if self.derived.contains_key(&name) {
                    return Err(RuntimeError::new(format!(
                        "restart checkpoint would duplicate derived state '{name}'"
                    )));
                }
                self.derived.insert(
                    name,
                    DerivedCell {
                        expression: member.expression.clone(),
                        value_type: member.value_type.clone(),
                        cached: None,
                        dependencies: HashSet::new(),
                        dependents: HashSet::new(),
                        evaluations: 0,
                        model_context: Some(context.clone()),
                    },
                );
            }

            for member in template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
            {
                let name = model_binding_name(identity, &member.name);
                let value = checkpoint.state_values.get(&name).cloned().ok_or_else(|| {
                    RuntimeError::new(format!(
                        "restart checkpoint is missing dynamic state '{name}'"
                    ))
                })?;
                if self.states.contains_key(&name) {
                    return Err(RuntimeError::new(format!(
                        "restart checkpoint would duplicate state '{name}'"
                    )));
                }
                self.states.insert(
                    name.clone(),
                    StateCell {
                        value: coerce_value(value, &member.value_type)?,
                        value_type: member.value_type.clone(),
                        designation: member.designation.clone(),
                        dependents: HashSet::new(),
                    },
                );
                consumed_states.insert(name);
            }
        }

        if checkpoint
            .state_values
            .keys()
            .any(|name| !consumed_states.contains(name))
        {
            let unexpected = checkpoint
                .state_values
                .keys()
                .find(|name| !consumed_states.contains(*name))
                .expect("unexpected checkpoint state should exist");
            return Err(RuntimeError::new(format!(
                "restart checkpoint contains state '{unexpected}' that the checked program cannot reconstruct"
            )));
        }

        self.next_dynamic_identity = checkpoint.next_dynamic_identity;
        Ok(())
    }
}

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state name = ""
    state folders: [live Folder] = []
    state documents: [live Document] = []
    state restoreParent: maybe live Folder = none
}

state workspace: Folder
state sourceFolder: maybe live Folder = none
state trashFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none

derived selectedInSource = selectedFolder is in sourceFolder.folders
derived selectedInTrash = selectedFolder is in trashFolder.folders
derived selectedName = selectedFolder.name
derived firstDocumentTitle = selectedFolder.documents[0].title

action seed {
    create Folder in workspace as source {
        through source.name = "Source"
        insert source into workspace.folders
    }
    create Folder in workspace as trash {
        through trash.name = "Trash"
        insert trash into workspace.folders
    }

    sourceFolder = workspace.folders[0]
    trashFolder = workspace.folders[1]

    create Folder in sourceFolder as folder {
        through folder.name = "Project"
        insert folder into sourceFolder.folders
    }
    selectedFolder = sourceFolder.folders[0]

    create Document in selectedFolder as document {
        through document.title = "Draft"
        insert document into selectedFolder.documents
    }
}

action moveToTrash {
    through selectedFolder.restoreParent = sourceFolder
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action failedRename {
    through selectedFolder.name = "Should Roll Back"
    fail "abort"
}

action restoreFromTrash {
    restoreDestination = selectedFolder.restoreParent
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    through selectedFolder.restoreParent = none
}

action staleOwnerProof {
    transfer selectedFolder from trashFolder to trashFolder
}

action currentOwnerProof {
    transfer selectedFolder from sourceFolder to sourceFolder
}

action createAnotherFolder {
    create Folder in workspace as another {
        through another.name = "Another"
        insert another into workspace.folders
    }
}
"#;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE).expect("restart pressure source should check")
}

fn seed_and_move_to_trash(runtime: &mut Runtime) {
    runtime.run_action("seed").expect("seed should commit");
    runtime
        .run_action("moveToTrash")
        .expect("Trash move of committed folder should commit");
}

#[test]
fn checkpoint_restores_committed_identity_state_structure_designation_and_provenance() {
    let checked = checked_source();
    let mut original =
        Runtime::from_checked_source(&checked).expect("original runtime should initialize");

    seed_and_move_to_trash(&mut original);
    original
        .run_action("failedRename")
        .expect_err("failed work must not become committed restart state");

    assert_eq!(
        original.value("selectedInSource").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        original.value("selectedInTrash").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        original.value("selectedName").unwrap(),
        Value::String("Project".to_string())
    );
    assert_eq!(
        original.value("firstDocumentTitle").unwrap(),
        Value::String("Draft".to_string())
    );
    assert_eq!(original.dynamic_model_types.len(), 4);
    assert_eq!(original.derived_evaluations("selectedInTrash"), Some(1));

    let checkpoint = original
        .capture_restart_checkpoint()
        .expect("committed world should checkpoint");
    let original_owners = original.dynamic_model_owners.clone();
    let original_types = original.dynamic_model_types.clone();
    let original_ids = original_types.keys().cloned().collect::<HashSet<_>>();

    let mut restarted =
        Runtime::from_checked_source(&checked).expect("fresh runtime should initialize");
    assert_eq!(restarted.derived_evaluations("selectedInTrash"), Some(0));
    restarted
        .restore_restart_checkpoint(&checkpoint)
        .expect("checkpoint should restore into the same checked program");

    assert_eq!(restarted.dynamic_model_owners, original_owners);
    assert_eq!(restarted.dynamic_model_types, original_types);
    assert_eq!(
        restarted.next_dynamic_identity,
        checkpoint.next_dynamic_identity
    );
    assert_eq!(
        restarted.value("selectedInSource").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        restarted.value("selectedInTrash").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(restarted.derived_evaluations("selectedInTrash"), Some(1));
    assert_eq!(
        restarted.value("selectedName").unwrap(),
        Value::String("Project".to_string())
    );
    assert_eq!(
        restarted.value("firstDocumentTitle").unwrap(),
        Value::String("Draft".to_string())
    );

    restarted
        .run_action("restoreFromTrash")
        .expect("restored model-local parent designation should drive restore");
    assert_eq!(
        restarted.value("selectedInSource").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        restarted.value("selectedInTrash").unwrap(),
        Value::Bool(false)
    );

    restarted
        .run_action("staleOwnerProof")
        .expect_err("old Trash owner must no longer prove provenance after restore");
    restarted
        .run_action("currentOwnerProof")
        .expect("restored source owner should prove current provenance");

    restarted
        .run_action("createAnotherFolder")
        .expect("creation after restart should commit");
    assert_eq!(restarted.dynamic_model_types.len(), original_ids.len() + 1);
    let new_ids = restarted
        .dynamic_model_types
        .keys()
        .filter(|identity| !original_ids.contains(*identity))
        .collect::<Vec<_>>();
    assert_eq!(new_ids.len(), 1);
    assert_eq!(
        restarted.next_dynamic_identity,
        checkpoint.next_dynamic_identity + 1
    );
}

fn run_action_with_durable_acceptance(
    runtime: &mut Runtime,
    checked: &crate::CheckedSource,
    name: &str,
    mut accept: impl FnMut(&RuntimeCheckpoint) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    if runtime.transaction.is_some() {
        return Err(RuntimeError::new(
            "durable action cannot start while a transaction is active",
        ));
    }

    let prior = runtime.capture_restart_checkpoint()?;
    runtime.transaction = Some(Transaction::default());
    let result = runtime.invoke_action(name, &[]);
    let transaction = match result {
        Ok(()) => runtime
            .transaction
            .take()
            .expect("successful durable action should retain its transaction"),
        Err(error) => {
            runtime.transaction = None;
            runtime.next_dynamic_identity = prior.next_dynamic_identity;
            return Err(error);
        }
    };

    let candidate_next_dynamic_identity = runtime.next_dynamic_identity;
    let mut candidate = Runtime::from_checked_source(checked)?;
    candidate.restore_restart_checkpoint(&prior)?;
    candidate.next_dynamic_identity = candidate_next_dynamic_identity;
    candidate.commit(transaction);
    let candidate_checkpoint = candidate.capture_restart_checkpoint()?;

    if let Err(error) = accept(&candidate_checkpoint) {
        runtime.next_dynamic_identity = prior.next_dynamic_identity;
        return Err(error);
    }

    *runtime = candidate;
    Ok(())
}

#[test]
fn durable_rejection_preserves_the_prior_committed_world_exactly() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let prior = runtime
        .capture_restart_checkpoint()
        .expect("prior committed world should checkpoint");

    let error =
        run_action_with_durable_acceptance(&mut runtime, &checked, "restoreFromTrash", |_| {
            Err(RuntimeError::new("durable provider rejected candidate"))
        })
        .expect_err("provider rejection must fail the action");
    assert!(error.message.contains("durable provider rejected"));

    let after = runtime
        .capture_restart_checkpoint()
        .expect("runtime should remain committed after rejection");
    assert_eq!(after, prior);
    assert_eq!(runtime.value("selectedInTrash").unwrap(), Value::Bool(true));
    assert_eq!(
        runtime.value("selectedInSource").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn durable_acceptance_publishes_exactly_the_accepted_candidate() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let mut accepted = None;

    run_action_with_durable_acceptance(&mut runtime, &checked, "restoreFromTrash", |checkpoint| {
        accepted = Some(checkpoint.clone());
        Ok(())
    })
    .expect("provider acceptance should publish the candidate world");

    let current = runtime
        .capture_restart_checkpoint()
        .expect("published world should checkpoint");
    assert_eq!(Some(current), accepted);
    assert_eq!(
        runtime.value("selectedInSource").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        runtime.value("selectedInTrash").unwrap(),
        Value::Bool(false)
    );
    runtime
        .run_action("currentOwnerProof")
        .expect("published candidate must include transferred provenance");
}

#[test]
fn semantic_failure_never_attempts_durable_acceptance() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let prior = runtime
        .capture_restart_checkpoint()
        .expect("prior committed world should checkpoint");
    let mut attempts = 0;

    run_action_with_durable_acceptance(&mut runtime, &checked, "failedRename", |_| {
        attempts += 1;
        Ok(())
    })
    .expect_err("semantic failure must abort before durable acceptance");

    assert_eq!(attempts, 0);
    assert_eq!(
        runtime.capture_restart_checkpoint().unwrap(),
        prior,
        "semantic failure must preserve the prior durable world",
    );
}

#[test]
fn rejected_dynamic_creation_does_not_publish_or_consume_restart_identity() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let prior = runtime
        .capture_restart_checkpoint()
        .expect("prior committed world should checkpoint");

    run_action_with_durable_acceptance(&mut runtime, &checked, "createAnotherFolder", |_| {
        Err(RuntimeError::new("storage unavailable"))
    })
    .expect_err("rejected candidate creation must fail");
    assert_eq!(runtime.capture_restart_checkpoint().unwrap(), prior);

    let existing_ids = runtime
        .dynamic_model_types
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    run_action_with_durable_acceptance(&mut runtime, &checked, "createAnotherFolder", |_| Ok(()))
        .expect("accepted retry should commit");

    let new_ids = runtime
        .dynamic_model_types
        .keys()
        .filter(|identity| !existing_ids.contains(*identity))
        .collect::<Vec<_>>();
    assert_eq!(new_ids.len(), 1);
    assert_eq!(
        runtime.next_dynamic_identity,
        prior.next_dynamic_identity + 1
    );
}

trait RuntimePersistenceProvider {
    fn load(&mut self) -> Result<Option<RuntimeCheckpoint>, RuntimeError>;
    fn replace(&mut self, checkpoint: &RuntimeCheckpoint) -> Result<(), RuntimeError>;
}

#[derive(Debug, Default)]
struct MemoryPersistenceProvider {
    current: Option<RuntimeCheckpoint>,
    reject_next_replace: bool,
    load_attempts: usize,
    replace_attempts: usize,
}

impl RuntimePersistenceProvider for MemoryPersistenceProvider {
    fn load(&mut self) -> Result<Option<RuntimeCheckpoint>, RuntimeError> {
        self.load_attempts += 1;
        Ok(self.current.clone())
    }

    fn replace(&mut self, checkpoint: &RuntimeCheckpoint) -> Result<(), RuntimeError> {
        self.replace_attempts += 1;
        if self.reject_next_replace {
            self.reject_next_replace = false;
            return Err(RuntimeError::new(
                "persistence provider rejected replacement",
            ));
        }
        self.current = Some(checkpoint.clone());
        Ok(())
    }
}

struct PersistentRuntime<P> {
    runtime: Runtime,
    checked: crate::CheckedSource,
    provider: P,
}

impl<P: RuntimePersistenceProvider> PersistentRuntime<P> {
    fn open(checked: crate::CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let mut runtime = Runtime::from_checked_source(&checked)?;
        if let Some(checkpoint) = provider.load()? {
            runtime.restore_restart_checkpoint(&checkpoint)?;
        }
        Ok(Self {
            runtime,
            checked,
            provider,
        })
    }

    fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        let provider = &mut self.provider;
        run_action_with_durable_acceptance(&mut self.runtime, &self.checked, name, |checkpoint| {
            provider.replace(checkpoint)
        })
    }
}

#[test]
fn empty_provider_opens_fresh_runtime_and_first_commit_becomes_durable() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut app = PersistentRuntime::open(checked, provider).expect("empty provider should open");

    assert_eq!(app.provider.load_attempts, 1);
    assert!(app.provider.current.is_none());

    app.run_action("seed")
        .expect("first durable action should commit");
    assert_eq!(app.provider.replace_attempts, 1);
    assert_eq!(
        app.provider.current,
        Some(app.runtime.capture_restart_checkpoint().unwrap())
    );
}

#[test]
fn provider_load_reconstructs_the_same_committed_world() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut first =
        PersistentRuntime::open(checked.clone(), provider).expect("runtime should open");
    first
        .run_action("seed")
        .expect("seed should commit durably");
    first
        .run_action("moveToTrash")
        .expect("Trash move should commit durably");
    let durable = first
        .provider
        .current
        .clone()
        .expect("provider should hold image");

    let provider = first.provider;
    let mut restarted =
        PersistentRuntime::open(checked, provider).expect("restart should load image");
    assert_eq!(restarted.provider.load_attempts, 2);
    assert_eq!(
        restarted.runtime.capture_restart_checkpoint().unwrap(),
        durable
    );
    assert_eq!(
        restarted.runtime.value("selectedInTrash").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        restarted.runtime.value("selectedInSource").unwrap(),
        Value::Bool(false)
    );

    restarted
        .run_action("restoreFromTrash")
        .expect("loaded restoreParent designation should drive durable restore");
    assert_eq!(
        restarted.provider.current,
        Some(restarted.runtime.capture_restart_checkpoint().unwrap())
    );
}

#[test]
fn provider_replace_failure_leaves_runtime_and_provider_on_same_prior_world() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut app = PersistentRuntime::open(checked, provider).expect("runtime should open");
    app.run_action("seed").expect("seed should commit durably");
    app.run_action("moveToTrash")
        .expect("Trash move should commit durably");
    let prior = app
        .provider
        .current
        .clone()
        .expect("prior durable image should exist");

    app.provider.reject_next_replace = true;
    app.run_action("restoreFromTrash")
        .expect_err("replace rejection must fail action");

    assert_eq!(app.provider.current, Some(prior.clone()));
    assert_eq!(app.runtime.capture_restart_checkpoint().unwrap(), prior);
    assert_eq!(
        app.runtime.value("selectedInTrash").unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn semantic_failure_does_not_call_provider_replace() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut app = PersistentRuntime::open(checked, provider).expect("runtime should open");
    app.run_action("seed").expect("seed should commit durably");
    let attempts = app.provider.replace_attempts;
    let prior = app
        .provider
        .current
        .clone()
        .expect("durable image should exist");

    app.run_action("failedRename")
        .expect_err("semantic failure must fail before persistence replacement");

    assert_eq!(app.provider.replace_attempts, attempts);
    assert_eq!(app.provider.current, Some(prior.clone()));
    assert_eq!(app.runtime.capture_restart_checkpoint().unwrap(), prior);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceShape {
    static_states: Vec<(String, String, Option<(String, bool)>)>,
    model_states: Vec<(String, String, String, Option<(String, bool)>)>,
    roots: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
struct CompatibleRuntimeImage {
    shape: PersistenceShape,
    checkpoint: RuntimeCheckpoint,
}

fn designation_shape(
    metadata: &Option<crate::designation_runtime_metadata::RuntimeDesignationMetadata>,
) -> Option<(String, bool)> {
    metadata
        .as_ref()
        .map(|metadata| (metadata.model_name.clone(), metadata.allows_none))
}

fn persistence_shape(checked: &crate::CheckedSource) -> Result<PersistenceShape, RuntimeError> {
    let runtime = Runtime::from_checked_source(checked)?;

    let mut static_states = runtime
        .states
        .iter()
        .map(|(name, state)| {
            (
                name.clone(),
                format!("{:?}", state.value_type),
                designation_shape(&state.designation),
            )
        })
        .collect::<Vec<_>>();
    static_states.sort();

    let mut model_states = checked
        .runtime_model_templates
        .values()
        .flat_map(|template| {
            template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
                .map(|member| {
                    (
                        template.name.clone(),
                        member.name.clone(),
                        format!("{:?}", member.value_type),
                        designation_shape(&member.designation),
                    )
                })
        })
        .collect::<Vec<_>>();
    model_states.sort();

    let mut roots = checked
        .runtime_model_roots
        .values()
        .map(|root| (root.name.clone(), root.model_name.clone()))
        .collect::<Vec<_>>();
    roots.sort();

    Ok(PersistenceShape {
        static_states,
        model_states,
        roots,
    })
}

fn capture_compatible_image(
    runtime: &Runtime,
    checked: &crate::CheckedSource,
) -> Result<CompatibleRuntimeImage, RuntimeError> {
    Ok(CompatibleRuntimeImage {
        shape: persistence_shape(checked)?,
        checkpoint: runtime.capture_restart_checkpoint()?,
    })
}

fn restore_compatible_image(
    runtime: &mut Runtime,
    checked: &crate::CheckedSource,
    image: &CompatibleRuntimeImage,
) -> Result<(), RuntimeError> {
    let current_shape = persistence_shape(checked)?;
    if current_shape != image.shape {
        return Err(RuntimeError::new(
            "persistence image is incompatible with the current checked application shape",
        ));
    }
    runtime.restore_restart_checkpoint(&image.checkpoint)
}

#[test]
fn identical_checked_shape_accepts_the_stored_image() {
    let checked = checked_source();
    let mut original = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut original);
    let image = capture_compatible_image(&original, &checked).expect("image should capture");

    let same_checked = checked_source();
    let mut restarted =
        Runtime::from_checked_source(&same_checked).expect("runtime should initialize");
    restore_compatible_image(&mut restarted, &same_checked, &image)
        .expect("identical checked shape should accept image");

    assert_eq!(
        restarted.capture_restart_checkpoint().unwrap(),
        image.checkpoint
    );
}

#[test]
fn incompatible_stored_state_shape_is_rejected_before_restore_mutation() {
    let checked = checked_source();
    let mut original = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut original);
    let image = capture_compatible_image(&original, &checked).expect("image should capture");

    let incompatible_source = SOURCE.replace(
        "state restoreParent: maybe live Folder = none",
        "state restoreParent: maybe live Folder = none\n    state archived = false",
    );
    let incompatible_checked = check_source_with_runtime_models(&incompatible_source)
        .expect("incompatible pressure source should still check");
    let mut restarted = Runtime::from_checked_source(&incompatible_checked)
        .expect("fresh incompatible runtime should initialize");
    let before = restarted
        .capture_restart_checkpoint()
        .expect("fresh runtime should checkpoint");

    let error = restore_compatible_image(&mut restarted, &incompatible_checked, &image)
        .expect_err("material state-shape mismatch must be rejected");
    assert!(error.message.contains("incompatible"));
    assert_eq!(
        restarted.capture_restart_checkpoint().unwrap(),
        before,
        "compatibility rejection must occur before restore mutation",
    );
}

#[test]
fn formatting_and_action_body_changes_do_not_change_persistence_shape() {
    let checked = checked_source();
    let formatted_source = format!("\n\n{}\n\n", SOURCE.replace("\n", "\n    "));
    let formatted = check_source_with_runtime_models(&formatted_source)
        .expect("formatting-only source should still check");
    assert_eq!(
        persistence_shape(&checked).unwrap(),
        persistence_shape(&formatted).unwrap(),
        "source formatting must not define persistence compatibility",
    );

    let changed_action_source = SOURCE.replace(
        "through another.name = \"Another\"",
        "through another.name = \"Another after upgrade\"",
    );
    let changed_action = check_source_with_runtime_models(&changed_action_source)
        .expect("action-body-only source should still check");
    assert_eq!(
        persistence_shape(&checked).unwrap(),
        persistence_shape(&changed_action).unwrap(),
        "action behavior may change without changing persisted reconstruction shape",
    );
}

#[test]
fn checkpoint_rejects_active_transaction_and_does_not_capture_staged_work() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);

    runtime.transaction = Some(Transaction::default());
    runtime
        .write_state(
            "__elanu_live$restoreDestination",
            Value::String("staged".to_string()),
        )
        .expect("test staging should write inside transaction");
    let error = runtime
        .capture_restart_checkpoint()
        .expect_err("active transaction must not checkpoint");
    assert!(error.message.contains("no active transaction"));
    runtime.transaction = None;

    let checkpoint = runtime
        .capture_restart_checkpoint()
        .expect("committed runtime should checkpoint after staged work is discarded");
    assert_ne!(
        checkpoint
            .state_values
            .get("__elanu_live$restoreDestination"),
        Some(&Value::String("staged".to_string()))
    );
}
