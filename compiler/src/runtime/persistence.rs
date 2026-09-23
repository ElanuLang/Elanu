use std::collections::{HashMap, HashSet};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceStateShape {
    name: String,
    value_type: ValueType,
    designation: Option<RuntimeDesignationMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceModelStateShape {
    model_name: String,
    member_name: String,
    value_type: ValueType,
    designation: Option<RuntimeDesignationMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceRootShape {
    root_name: String,
    model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceShape {
    static_states: Vec<PersistenceStateShape>,
    model_states: Vec<PersistenceModelStateShape>,
    roots: Vec<PersistenceRootShape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceDynamicModel {
    model_name: String,
    owner: String,
}

/// Opaque runtime-owned image of one committed Elanu application world.
///
/// Providers may clone, store, and replace this value, but its internal encoding
/// is intentionally not part of the host contract.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistenceImage {
    shape: PersistenceShape,
    state_values: HashMap<String, Value>,
    dynamic_models: HashMap<String, PersistenceDynamicModel>,
    next_dynamic_identity: u64,
}

/// Synchronous host boundary for durable Elanu runtime state.
///
/// `replace` must either accept the complete candidate image or fail without
/// changing the previously stored image.
pub trait PersistenceProvider {
    fn load(&mut self) -> Result<Option<PersistenceImage>, RuntimeError>;
    fn replace(&mut self, image: &PersistenceImage) -> Result<(), RuntimeError>;
}

/// Runtime wrapper that makes durable provider acceptance part of top-level
/// action publication while preserving ordinary `Runtime` behavior separately.
pub struct PersistentRuntime<P> {
    runtime: Runtime,
    checked: CheckedSource,
    shape: PersistenceShape,
    provider: P,
}

impl<P: PersistenceProvider> PersistentRuntime<P> {
    pub fn open(checked: CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let shape = persistence_shape(&checked)?;
        let mut runtime = Runtime::from_checked_source(&checked)?;

        if let Some(image) = provider.load()? {
            restore_image(&mut runtime, &shape, &image)?;
        }

        Ok(Self {
            runtime,
            checked,
            shape,
            provider,
        })
    }

    pub fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        if self.runtime.transaction.is_some() {
            return Err(RuntimeError::new(
                "persistent runtime cannot start a top-level action while a transaction is active",
            ));
        }

        let prior = capture_image(&self.runtime, &self.shape)?;
        self.runtime.transaction = Some(Transaction::default());
        let result = self.runtime.invoke_action(name, &[]);
        let transaction = match result {
            Ok(()) => self
                .runtime
                .transaction
                .take()
                .expect("successful persistent action should retain its transaction"),
            Err(error) => {
                self.runtime.transaction = None;
                self.runtime.next_dynamic_identity = prior.next_dynamic_identity;
                return Err(error);
            }
        };

        let candidate_next_dynamic_identity = self.runtime.next_dynamic_identity;
        let mut candidate = Runtime::from_checked_source(&self.checked)?;
        restore_image(&mut candidate, &self.shape, &prior)?;
        candidate.next_dynamic_identity = candidate_next_dynamic_identity;
        candidate.commit(transaction);
        let candidate_image = capture_image(&candidate, &self.shape)?;

        if let Err(error) = self.provider.replace(&candidate_image) {
            self.runtime.next_dynamic_identity = prior.next_dynamic_identity;
            return Err(error);
        }

        self.runtime = candidate;
        Ok(())
    }

    pub fn value(&mut self, name: &str) -> Result<Value, RuntimeError> {
        self.runtime.value(name)
    }

    pub fn snapshot(&mut self) -> Result<Vec<SnapshotEntry>, RuntimeError> {
        self.runtime.snapshot()
    }

    pub fn derived_evaluations(&self, name: &str) -> Option<usize> {
        self.runtime.derived_evaluations(name)
    }

    pub fn into_provider(self) -> P {
        self.provider
    }
}

fn persistence_shape(checked: &CheckedSource) -> Result<PersistenceShape, RuntimeError> {
    let runtime = Runtime::from_checked_source(checked)?;

    let mut static_states = runtime
        .states
        .iter()
        .map(|(name, state)| PersistenceStateShape {
            name: name.clone(),
            value_type: state.value_type.clone(),
            designation: state.designation.clone(),
        })
        .collect::<Vec<_>>();
    static_states.sort_by(|left, right| left.name.cmp(&right.name));

    let mut model_states = checked
        .runtime_model_templates
        .values()
        .flat_map(|template| {
            template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
                .map(|member| PersistenceModelStateShape {
                    model_name: template.name.clone(),
                    member_name: member.name.clone(),
                    value_type: member.value_type.clone(),
                    designation: member.designation.clone(),
                })
        })
        .collect::<Vec<_>>();
    model_states.sort_by(|left, right| {
        (&left.model_name, &left.member_name).cmp(&(&right.model_name, &right.member_name))
    });

    let mut roots = checked
        .runtime_model_roots
        .values()
        .map(|root| PersistenceRootShape {
            root_name: root.name.clone(),
            model_name: root.model_name.clone(),
        })
        .collect::<Vec<_>>();
    roots.sort_by(|left, right| left.root_name.cmp(&right.root_name));

    Ok(PersistenceShape {
        static_states,
        model_states,
        roots,
    })
}

fn capture_image(
    runtime: &Runtime,
    shape: &PersistenceShape,
) -> Result<PersistenceImage, RuntimeError> {
    if runtime.transaction.is_some() {
        return Err(RuntimeError::new(
            "persistence image requires a committed runtime with no active transaction",
        ));
    }

    if runtime.dynamic_model_owners.len() != runtime.dynamic_model_types.len() {
        return Err(RuntimeError::new(
            "dynamic modeled identity metadata is incomplete",
        ));
    }

    let mut dynamic_models = HashMap::new();
    for (identity, model_name) in &runtime.dynamic_model_types {
        let owner = runtime.dynamic_model_owners.get(identity).ok_or_else(|| {
            RuntimeError::new(format!(
                "dynamic modeled identity '{identity}' is missing owner provenance"
            ))
        })?;
        dynamic_models.insert(
            identity.clone(),
            PersistenceDynamicModel {
                model_name: model_name.clone(),
                owner: owner.clone(),
            },
        );
    }

    Ok(PersistenceImage {
        shape: shape.clone(),
        state_values: runtime
            .states
            .iter()
            .map(|(name, cell)| (name.clone(), cell.value.clone()))
            .collect(),
        dynamic_models,
        next_dynamic_identity: runtime.next_dynamic_identity,
    })
}

fn restore_image(
    runtime: &mut Runtime,
    shape: &PersistenceShape,
    image: &PersistenceImage,
) -> Result<(), RuntimeError> {
    if &image.shape != shape {
        return Err(RuntimeError::new(
            "persistence image is incompatible with the current checked application shape",
        ));
    }
    if runtime.transaction.is_some() {
        return Err(RuntimeError::new(
            "persistence image cannot restore into an active transaction",
        ));
    }
    if !runtime.dynamic_model_owners.is_empty() || !runtime.dynamic_model_types.is_empty() {
        return Err(RuntimeError::new(
            "persistence image currently restores only into a fresh runtime",
        ));
    }

    let mut consumed_states = HashSet::new();
    let static_state_names = runtime.states.keys().cloned().collect::<Vec<_>>();
    for name in static_state_names {
        let value = image.state_values.get(&name).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "persistence image is missing committed state '{name}'"
            ))
        })?;
        let state = runtime
            .states
            .get_mut(&name)
            .expect("fresh runtime state should still exist");
        state.value = coerce_value(value, &state.value_type)?;
        state.dependents.clear();
        consumed_states.insert(name);
    }

    for (identity, dynamic) in &image.dynamic_models {
        if !runtime
            .runtime_model_templates
            .contains_key(&dynamic.model_name)
        {
            return Err(RuntimeError::new(format!(
                "persistence image references unknown state model '{}'",
                dynamic.model_name
            )));
        }
        let owner_exists = runtime.runtime_model_roots.contains_key(&dynamic.owner)
            || image.dynamic_models.contains_key(&dynamic.owner);
        if !owner_exists {
            return Err(RuntimeError::new(format!(
                "persistence image owner '{}' for '{}' is not part of the checked application world",
                dynamic.owner, identity
            )));
        }
    }

    runtime.dynamic_model_owners = image
        .dynamic_models
        .iter()
        .map(|(identity, dynamic)| (identity.clone(), dynamic.owner.clone()))
        .collect();
    runtime.dynamic_model_types = image
        .dynamic_models
        .iter()
        .map(|(identity, dynamic)| (identity.clone(), dynamic.model_name.clone()))
        .collect();

    for (identity, dynamic) in &image.dynamic_models {
        let template = runtime
            .runtime_model_templates
            .get(&dynamic.model_name)
            .cloned()
            .expect("persistence image model existence was validated");
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
            if runtime.derived.contains_key(&name) {
                return Err(RuntimeError::new(format!(
                    "persistence image would duplicate derived state '{name}'"
                )));
            }
            runtime.derived.insert(
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
            let value = image.state_values.get(&name).cloned().ok_or_else(|| {
                RuntimeError::new(format!(
                    "persistence image is missing dynamic state '{name}'"
                ))
            })?;
            if runtime.states.contains_key(&name) {
                return Err(RuntimeError::new(format!(
                    "persistence image would duplicate state '{name}'"
                )));
            }
            runtime.states.insert(
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

    if let Some(unexpected) = image
        .state_values
        .keys()
        .find(|name| !consumed_states.contains(*name))
    {
        return Err(RuntimeError::new(format!(
            "persistence image contains state '{unexpected}' that the checked program cannot reconstruct"
        )));
    }

    runtime.next_dynamic_identity = image.next_dynamic_identity;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_source_with_runtime_models;

    #[derive(Default)]
    struct MemoryProvider {
        image: Option<PersistenceImage>,
        reject_next_replace: bool,
        load_attempts: usize,
        replace_attempts: usize,
    }

    impl PersistenceProvider for MemoryProvider {
        fn load(&mut self) -> Result<Option<PersistenceImage>, RuntimeError> {
            self.load_attempts += 1;
            Ok(self.image.clone())
        }

        fn replace(&mut self, image: &PersistenceImage) -> Result<(), RuntimeError> {
            self.replace_attempts += 1;
            if self.reject_next_replace {
                self.reject_next_replace = false;
                return Err(RuntimeError::new("provider rejected replacement"));
            }
            self.image = Some(image.clone());
            Ok(())
        }
    }

    const SOURCE: &str = r#"
state model Folder {
    state name = ""
    state folders: [live Folder] = []
}

state workspace: Folder
state selected: maybe live Folder = none

derived selectedName = selected.name

action createFolder {
    create Folder in workspace as folder {
        through folder.name = "Project"
        insert folder into workspace.folders
    }
    selected = workspace.folders[0]
}

action rename {
    through selected.name = "Renamed"
}

action failedRename {
    through selected.name = "Wrong"
    fail "abort"
}
"#;

    fn checked() -> CheckedSource {
        check_source_with_runtime_models(SOURCE).expect("persistence source should check")
    }

    #[test]
    fn empty_provider_opens_fresh_and_first_action_becomes_durable() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default())
            .expect("persistent runtime should open");

        assert_eq!(app.provider.load_attempts, 1);
        app.run_action("createFolder")
            .expect("first action should commit durably");
        assert_eq!(app.provider.replace_attempts, 1);
        assert_eq!(
            app.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
        assert!(app.provider.image.is_some());
    }

    #[test]
    fn restart_loads_committed_world_and_rebuilds_derived_state() {
        let mut first = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        assert_eq!(first.derived_evaluations("selectedName"), Some(0));
        assert_eq!(
            first.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
        let provider = first.into_provider();

        let mut restarted = PersistentRuntime::open(checked(), provider).unwrap();
        assert_eq!(restarted.derived_evaluations("selectedName"), Some(0));
        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
    }

    #[test]
    fn provider_rejection_preserves_prior_runtime_and_durable_world() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        app.run_action("createFolder").unwrap();
        let prior = app.provider.image.clone().unwrap();
        app.provider.reject_next_replace = true;

        app.run_action("rename")
            .expect_err("provider rejection must fail action");

        assert_eq!(app.provider.image, Some(prior));
        assert_eq!(
            app.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
    }

    #[test]
    fn semantic_failure_never_attempts_provider_replacement() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        app.run_action("createFolder").unwrap();
        let attempts = app.provider.replace_attempts;

        app.run_action("failedRename")
            .expect_err("semantic failure must fail action");

        assert_eq!(app.provider.replace_attempts, attempts);
        assert_eq!(
            app.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
    }

    #[test]
    fn incompatible_image_is_rejected_before_startup_restore() {
        let mut first = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        let provider = first.into_provider();

        let incompatible_source = SOURCE.replace(
            "state name = \"\"",
            "state name = \"\"\n    state archived = false",
        );
        let incompatible = check_source_with_runtime_models(&incompatible_source)
            .expect("incompatible source should still check");
        let error = PersistentRuntime::open(incompatible, provider)
            .err()
            .expect("incompatible image must be rejected");

        assert!(error.message.contains("incompatible"));
    }

    #[test]
    fn action_body_change_with_same_persisted_shape_remains_compatible() {
        let mut first = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        let provider = first.into_provider();

        let changed_source = SOURCE.replace("\"Renamed\"", "\"Renamed after upgrade\"");
        let changed = check_source_with_runtime_models(&changed_source)
            .expect("action-body-only change should check");
        let mut restarted = PersistentRuntime::open(changed, provider)
            .expect("same persisted reconstruction shape should remain compatible");

        restarted.run_action("rename").unwrap();
        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Renamed after upgrade".to_string())
        );
    }
}
