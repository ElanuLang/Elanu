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
            if !self.runtime_model_templates.contains_key(&dynamic.model_name) {
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

    assert_eq!(original.value("selectedInSource").unwrap(), Value::Bool(false));
    assert_eq!(original.value("selectedInTrash").unwrap(), Value::Bool(true));
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
    assert_eq!(restarted.next_dynamic_identity, checkpoint.next_dynamic_identity);
    assert_eq!(restarted.value("selectedInSource").unwrap(), Value::Bool(false));
    assert_eq!(restarted.value("selectedInTrash").unwrap(), Value::Bool(true));
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
    assert_eq!(restarted.value("selectedInSource").unwrap(), Value::Bool(true));
    assert_eq!(restarted.value("selectedInTrash").unwrap(), Value::Bool(false));

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
    assert_eq!(restarted.next_dynamic_identity, checkpoint.next_dynamic_identity + 1);
}

#[test]
fn checkpoint_rejects_active_transaction_and_does_not_capture_staged_work() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);

    runtime.transaction = Some(Transaction::default());
    runtime
        .write_state("__meld_live$restoreDestination", Value::String("staged".to_string()))
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
            .get("__meld_live$restoreDestination"),
        Some(&Value::String("staged".to_string()))
    );
}
