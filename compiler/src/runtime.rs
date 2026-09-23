use caseless::default_case_fold_str;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ast::{
    ActionArgument, ActionDecl, ActionParameterKind, AssignmentOperator, BinaryOperator,
    Declaration, Expr, Statement,
};
use crate::designation_runtime_metadata::RuntimeDesignationMetadata;
use crate::reduction_surface::ReductionSpec;
use crate::relative_navigation::{
    resolve_unique_neighbor, resolve_unique_occurrence, RelativeDirection, RelativeNeighborError,
    UniqueOccurrenceError,
};
use crate::runtime_model_templates::{
    RuntimeModelMemberKind, RuntimeModelRoot, RuntimeModelTemplate,
};
use crate::runtime_sequence_markers::decode_runtime_sequence_value;
use crate::semantic::{show_type, CheckedProgram, ValueType};
use crate::sequence_surface::decode_sequence_literal;
use crate::structural_move::{move_unique_relative, RelativePlacement, StructuralMoveError};
use crate::CheckedSource;

#[cfg(test)]
mod designation_metadata_execution;
#[cfg(test)]
mod lifetime_termination_execution;
#[cfg(test)]
mod model_local_designation_experiment;
pub mod persistence;
#[cfg(test)]
mod persistence_identity_metadata;
#[cfg(test)]
mod provenance_transfer_execution;
#[cfg(test)]
mod restart_checkpoint_experiment;
#[cfg(test)]
mod structural_move_execution;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Sequence {
        element_model: String,
        targets: Vec<String>,
    },
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
            Value::Bool(value) => write!(f, "{value}"),
            Value::String(value) => write!(f, "{value:?}"),
            Value::Sequence { targets, .. } => {
                write!(f, "[")?;
                for (index, target) in targets.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "live {target}")?;
                }
                write!(f, "]")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    State,
    Derived,
}

impl fmt::Display for BindingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingKind::State => write!(f, "state"),
            BindingKind::Derived => write!(f, "derived"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotEntry {
    pub kind: BindingKind,
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub message: String,
}

impl RuntimeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Debug, Clone)]
struct StateCell {
    value: Value,
    value_type: ValueType,
    designation: Option<RuntimeDesignationMetadata>,
    dependents: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ModelRuntimeContext {
    root: String,
    members: HashSet<String>,
}

#[derive(Debug, Clone)]
struct DerivedCell {
    expression: Expr,
    value_type: ValueType,
    cached: Option<Value>,
    dependencies: HashSet<String>,
    dependents: HashSet<String>,
    evaluations: usize,
    model_context: Option<ModelRuntimeContext>,
}

#[derive(Debug, Default)]
struct Transaction {
    writes: HashMap<String, Value>,
    derived_values: HashMap<String, Value>,
    derived_dependencies: HashMap<String, HashSet<String>>,
    dependents: HashMap<String, HashSet<String>>,
    created_states: HashMap<String, StateCell>,
    created_derived: HashMap<String, DerivedCell>,
    created_model_owners: HashMap<String, String>,
    created_model_types: HashMap<String, String>,
    updated_model_owners: HashMap<String, String>,
    deleted_states: HashSet<String>,
    deleted_derived: HashSet<String>,
    terminated_model_identities: HashSet<String>,
}

#[derive(Debug, Clone)]
enum ActionBinding {
    Value(Value),
    State(String),
}

#[derive(Debug, Clone, Default)]
struct ActionFrame {
    bindings: HashMap<String, ActionBinding>,
}

#[derive(Debug, Clone)]
struct RuntimeReduction {
    spec: ReductionSpec,
    result_type: ValueType,
    initial: Expr,
    step: Expr,
}

#[derive(Debug, Clone)]
struct ReductionFrame {
    accumulator_name: String,
    accumulator: Value,
}

#[derive(Debug, Clone)]
struct ElementFrame {
    element_name: String,
    target: String,
}

#[derive(Debug)]
pub struct Runtime {
    states: HashMap<String, StateCell>,
    derived: HashMap<String, DerivedCell>,
    actions: HashMap<String, ActionDecl>,
    action_parameter_types: HashMap<String, Vec<ValueType>>,
    binding_order: Vec<(BindingKind, String)>,
    eval_stack: Vec<String>,
    action_stack: Vec<String>,
    action_frames: Vec<ActionFrame>,
    reductions: HashMap<String, RuntimeReduction>,
    reduction_frames: Vec<ReductionFrame>,
    element_frames: Vec<ElementFrame>,
    model_frames: Vec<ModelRuntimeContext>,
    runtime_model_templates: HashMap<String, RuntimeModelTemplate>,
    runtime_model_roots: HashMap<String, RuntimeModelRoot>,
    runtime_designations: HashMap<String, RuntimeDesignationMetadata>,
    dynamic_model_owners: HashMap<String, String>,
    dynamic_model_types: HashMap<String, String>,
    runtime_index_grant_carriers: HashMap<String, Expr>,
    next_dynamic_identity: u64,
    transaction: Option<Transaction>,
}

impl Runtime {
    pub fn from_program(program: &CheckedProgram) -> Result<Self, RuntimeError> {
        let mut runtime = Self {
            states: HashMap::new(),
            derived: HashMap::new(),
            actions: HashMap::new(),
            action_parameter_types: HashMap::new(),
            binding_order: Vec::new(),
            eval_stack: Vec::new(),
            action_stack: Vec::new(),
            action_frames: Vec::new(),
            reductions: HashMap::new(),
            reduction_frames: Vec::new(),
            element_frames: Vec::new(),
            model_frames: Vec::new(),
            runtime_model_templates: HashMap::new(),
            runtime_model_roots: HashMap::new(),
            runtime_designations: HashMap::new(),
            dynamic_model_owners: HashMap::new(),
            dynamic_model_types: HashMap::new(),
            runtime_index_grant_carriers: HashMap::new(),
            next_dynamic_identity: 0,
            transaction: None,
        };

        for declaration in &program.program.declarations {
            match declaration {
                Declaration::State(state) => {
                    if state.name.starts_with(
                        crate::runtime_index_grant_transport::RUNTIME_INDEX_GRANT_CARRIER_PREFIX,
                    ) {
                        program.binding_type(&state.name).cloned().ok_or_else(|| {
                            RuntimeError::new(format!(
                                "checked type information missing for runtime indexed grant carrier '{}'",
                                state.name
                            ))
                        })?;
                        if !matches!(
                            state.initializer,
                            Expr::RuntimeIndexMember { .. } | Expr::RuntimeDesignationMember { .. }
                        ) {
                            return Err(RuntimeError::new(format!(
                                "runtime indexed grant carrier '{}' is malformed",
                                state.name
                            )));
                        }
                        runtime
                            .runtime_index_grant_carriers
                            .insert(state.name.clone(), state.initializer.clone());
                        continue;
                    }

                    let value = runtime.eval_expr(&state.initializer, None)?;
                    let value_type =
                        program.binding_type(&state.name).cloned().ok_or_else(|| {
                            RuntimeError::new(format!(
                                "checked type information missing for state '{}'",
                                state.name
                            ))
                        })?;
                    let value = coerce_value(value, &value_type)?;
                    runtime
                        .binding_order
                        .push((BindingKind::State, state.name.clone()));
                    runtime.states.insert(
                        state.name.clone(),
                        StateCell {
                            value,
                            value_type,
                            designation: None,
                            dependents: HashSet::new(),
                        },
                    );
                }
                Declaration::Derived(derived) => {
                    if let Some(payload) = program.runtime_reduction(&derived.name) {
                        runtime.reductions.insert(
                            derived.name.clone(),
                            RuntimeReduction {
                                spec: payload.reduction.clone(),
                                result_type: payload.result_type.clone(),
                                initial: payload.initial.clone(),
                                step: payload.step.clone(),
                            },
                        );
                    }

                    let value_type =
                        program
                            .binding_type(&derived.name)
                            .cloned()
                            .ok_or_else(|| {
                                RuntimeError::new(format!(
                                    "checked type information missing for derived '{}'",
                                    derived.name
                                ))
                            })?;
                    runtime
                        .binding_order
                        .push((BindingKind::Derived, derived.name.clone()));
                    runtime.derived.insert(
                        derived.name.clone(),
                        DerivedCell {
                            expression: derived.expression.clone(),
                            value_type,
                            cached: None,
                            dependencies: HashSet::new(),
                            dependents: HashSet::new(),
                            evaluations: 0,
                            model_context: None,
                        },
                    );
                }
                Declaration::Action(action) => {
                    let parameter_types = program
                        .action_parameter_types(&action.name)
                        .ok_or_else(|| {
                            RuntimeError::new(format!(
                                "checked parameter type information missing for action '{}'",
                                action.name
                            ))
                        })?
                        .to_vec();

                    runtime
                        .action_parameter_types
                        .insert(action.name.clone(), parameter_types);
                    runtime.actions.insert(action.name.clone(), action.clone());
                }
            }
        }

        Ok(runtime)
    }

    pub fn from_checked_source(source: &CheckedSource) -> Result<Self, RuntimeError> {
        let mut runtime = Self::from_program(&source.program)?;
        runtime.runtime_model_templates = source.runtime_model_templates.clone();
        runtime.runtime_model_roots = source.runtime_model_roots.clone();
        runtime.runtime_designations = source.runtime_designations.clone();
        for (name, metadata) in &source.runtime_designations {
            let state = runtime.states.get_mut(name).ok_or_else(|| {
                RuntimeError::new(format!(
                    "persistent designation metadata names missing state '{name}'"
                ))
            })?;
            state.designation = Some(metadata.clone());
        }
        Ok(runtime)
    }

    pub fn runtime_model_template(&self, name: &str) -> Option<&RuntimeModelTemplate> {
        self.runtime_model_templates.get(name)
    }

    pub fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        if self.transaction.is_some() {
            return Err(RuntimeError::new(
                "run_action cannot start a new top-level action while a transaction is active",
            ));
        }

        self.transaction = Some(Transaction::default());
        let result = self.invoke_action(name, &[]);

        match result {
            Ok(()) => {
                let transaction = self.transaction.take().expect("transaction should exist");
                self.commit(transaction);
                Ok(())
            }
            Err(error) => {
                self.transaction = None;
                Err(error)
            }
        }
    }

    pub fn value(&mut self, name: &str) -> Result<Value, RuntimeError> {
        self.read_name(name, None)
    }

    pub fn derived_evaluations(&self, name: &str) -> Option<usize> {
        self.derived.get(name).map(|cell| cell.evaluations)
    }

    pub fn snapshot(&mut self) -> Result<Vec<SnapshotEntry>, RuntimeError> {
        let mut entries = Vec::new();

        for (kind, name) in self.binding_order.clone() {
            entries.push(SnapshotEntry {
                kind,
                value: self.read_name(&name, None)?,
                name,
            });
        }

        Ok(entries)
    }

    pub fn instantiate_runtime_model(
        &mut self,
        model_name: &str,
        owner_root: &str,
    ) -> Result<String, RuntimeError> {
        if self.transaction.is_none() {
            return Err(RuntimeError::new(
                "runtime model identity can only be created inside an active action transaction",
            ));
        }

        if !self.model_identity_exists(owner_root) {
            return Err(RuntimeError::new(format!(
                "unknown modeled-state owner '{owner_root}'"
            )));
        }

        let template = self
            .runtime_model_templates
            .get(model_name)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown runtime model '{model_name}'")))?;

        let identity = format!(
            "__meld_dynamic${}${}",
            template.name, self.next_dynamic_identity
        );
        self.next_dynamic_identity = self
            .next_dynamic_identity
            .checked_add(1)
            .ok_or_else(|| RuntimeError::new("dynamic model identity space exhausted"))?;

        self.transaction
            .as_mut()
            .expect("transaction should exist while creating model")
            .created_model_owners
            .insert(identity.clone(), owner_root.to_string());
        self.transaction
            .as_mut()
            .expect("transaction should exist while creating model")
            .created_model_types
            .insert(identity.clone(), model_name.to_string());

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
            let name = model_binding_name(&identity, &member.name);
            let cell = DerivedCell {
                expression: member.expression.clone(),
                value_type: member.value_type.clone(),
                cached: None,
                dependencies: HashSet::new(),
                dependents: HashSet::new(),
                evaluations: 0,
                model_context: Some(context.clone()),
            };
            self.transaction
                .as_mut()
                .expect("transaction should exist while creating model")
                .created_derived
                .insert(name, cell);
        }

        for member in template
            .members
            .iter()
            .filter(|member| member.kind == RuntimeModelMemberKind::State)
        {
            let value = if let (ValueType::SequenceLive(element_model), Expr::String(encoded)) =
                (&member.value_type, &member.expression)
            {
                if let Some(targets) = decode_sequence_literal(encoded) {
                    if !targets.is_empty() {
                        return Err(RuntimeError::new(
                            "runtime-instantiated model sequence defaults must be empty",
                        ));
                    }
                    Value::Sequence {
                        element_model: element_model.clone(),
                        targets,
                    }
                } else {
                    self.model_frames.push(context.clone());
                    let result = self.eval_expr(&member.expression, None);
                    self.model_frames
                        .pop()
                        .expect("runtime model evaluation frame should exist");
                    coerce_value(result?, &member.value_type)?
                }
            } else {
                self.model_frames.push(context.clone());
                let result = self.eval_expr(&member.expression, None);
                self.model_frames
                    .pop()
                    .expect("runtime model evaluation frame should exist");
                coerce_value(result?, &member.value_type)?
            };
            let name = model_binding_name(&identity, &member.name);
            let cell = StateCell {
                value,
                value_type: member.value_type.clone(),
                designation: member.designation.clone(),
                dependents: HashSet::new(),
            };
            self.transaction
                .as_mut()
                .expect("transaction should exist while creating model")
                .created_states
                .insert(name, cell);
        }

        Ok(identity)
    }

    fn model_identity_exists(&self, root: &str) -> bool {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.terminated_model_identities.contains(root))
        {
            return false;
        }

        self.runtime_model_roots.contains_key(root)
            || self.dynamic_model_owners.contains_key(root)
            || self
                .transaction
                .as_ref()
                .is_some_and(|transaction| transaction.created_model_owners.contains_key(root))
    }

    fn current_dynamic_model_owner(&self, identity: &str) -> Option<String> {
        let transaction = self.transaction.as_ref();
        if transaction
            .is_some_and(|transaction| transaction.terminated_model_identities.contains(identity))
        {
            return None;
        }

        transaction
            .and_then(|transaction| transaction.updated_model_owners.get(identity))
            .or_else(|| {
                transaction.and_then(|transaction| transaction.created_model_owners.get(identity))
            })
            .or_else(|| self.dynamic_model_owners.get(identity))
            .cloned()
    }

    fn has_live_dynamic_child_rooted_in(&self, owner: &str) -> bool {
        let committed_child = self
            .dynamic_model_owners
            .keys()
            .any(|child| self.current_dynamic_model_owner(child).as_deref() == Some(owner));
        if committed_child {
            return true;
        }

        self.transaction.as_ref().is_some_and(|transaction| {
            transaction
                .created_model_owners
                .iter()
                .any(|(child, child_owner)| {
                    !transaction.terminated_model_identities.contains(child) && child_owner == owner
                })
        })
    }

    fn transfer_runtime_model_owner(
        &mut self,
        identity: &str,
        expected_owner: &str,
        destination_owner: &str,
    ) -> Result<(), RuntimeError> {
        let Some(transaction) = self.transaction.as_ref() else {
            return Err(RuntimeError::new(
                "runtime model provenance can only transfer inside an active action transaction",
            ));
        };

        if transaction.terminated_model_identities.contains(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child is already terminated in this transaction",
            ));
        }
        if !self.dynamic_model_owners.contains_key(identity) {
            return Err(RuntimeError::new(
                "runtime provenance transfer requires an existing committed dynamic child",
            ));
        }

        let actual_owner = self
            .current_dynamic_model_owner(identity)
            .expect("committed live dynamic child should have an owner");
        if actual_owner != expected_owner {
            return Err(RuntimeError::new(format!(
                "runtime modeled-state child is currently rooted in '{actual_owner}', not expected owner '{expected_owner}'"
            )));
        }

        if identity == destination_owner {
            return Err(RuntimeError::new(
                "runtime modeled-state child cannot become its own rooting owner",
            ));
        }
        if !self.model_identity_exists(destination_owner) {
            return Err(RuntimeError::new(format!(
                "unknown modeled-state destination owner '{destination_owner}'"
            )));
        }
        if self.transfer_destination_would_create_provenance_cycle(identity, destination_owner) {
            return Err(RuntimeError::new(
                "runtime modeled-state provenance transfer would create an owner cycle",
            ));
        }

        if actual_owner == destination_owner {
            return Ok(());
        }

        self.transaction
            .as_mut()
            .expect("transaction should exist while transferring model provenance")
            .updated_model_owners
            .insert(identity.to_string(), destination_owner.to_string());
        Ok(())
    }

    fn transfer_destination_would_create_provenance_cycle(
        &self,
        identity: &str,
        destination_owner: &str,
    ) -> bool {
        let mut current = destination_owner.to_string();
        let mut visited = HashSet::new();

        loop {
            if current == identity {
                return true;
            }
            if !visited.insert(current.clone()) {
                return true;
            }
            let Some(owner) = self.current_dynamic_model_owner(&current) else {
                return false;
            };
            current = owner;
        }
    }

    fn committed_provenance_subtree_postorder(
        &self,
        root: &str,
    ) -> Result<Vec<String>, RuntimeError> {
        if !self.dynamic_model_owners.contains_key(root) {
            return Err(RuntimeError::new(
                "runtime subtree purge requires an existing committed dynamic root",
            ));
        }

        if let Some(transaction) = self.transaction.as_ref() {
            for fresh in transaction.created_model_owners.keys() {
                if transaction.terminated_model_identities.contains(fresh) {
                    continue;
                }
                let mut current = fresh.clone();
                let mut visited = HashSet::new();
                loop {
                    if !visited.insert(current.clone()) {
                        break;
                    }
                    let Some(owner) = self.current_dynamic_model_owner(&current) else {
                        break;
                    };
                    if owner == root {
                        return Err(RuntimeError::new(
                            "runtime subtree purge does not yet cancel fresh transaction-local descendants",
                        ));
                    }
                    current = owner;
                }
            }
        }

        fn visit(
            runtime: &Runtime,
            identity: &str,
            visited: &mut HashSet<String>,
            postorder: &mut Vec<String>,
        ) {
            if !visited.insert(identity.to_string()) {
                return;
            }
            let children = runtime
                .dynamic_model_owners
                .keys()
                .filter(|child| {
                    runtime.current_dynamic_model_owner(child).as_deref() == Some(identity)
                })
                .cloned()
                .collect::<Vec<_>>();
            for child in children {
                visit(runtime, &child, visited, postorder);
            }
            postorder.push(identity.to_string());
        }

        let mut visited = HashSet::new();
        let mut postorder = Vec::new();
        visit(self, root, &mut visited, &mut postorder);
        Ok(postorder)
    }

    fn terminate_runtime_model_subtree(
        &mut self,
        identity: &str,
        claimed_owner: &str,
    ) -> Result<(), RuntimeError> {
        let actual_owner = self.current_dynamic_model_owner(identity).ok_or_else(|| {
            RuntimeError::new(
                "runtime subtree purge requires an existing live committed dynamic root",
            )
        })?;
        if actual_owner != claimed_owner {
            return Err(RuntimeError::new(format!(
                "runtime subtree purge requires rooting owner '{actual_owner}', not '{claimed_owner}'"
            )));
        }

        let postorder = self.committed_provenance_subtree_postorder(identity)?;
        for target in postorder {
            let owner = self.current_dynamic_model_owner(&target).ok_or_else(|| {
                RuntimeError::new(format!(
                    "runtime subtree purge lost current owner for '{target}'"
                ))
            })?;
            self.terminate_runtime_model(&target, &owner)?;
        }
        Ok(())
    }

    fn terminate_runtime_model(
        &mut self,
        identity: &str,
        claimed_owner: &str,
    ) -> Result<(), RuntimeError> {
        let Some(transaction) = self.transaction.as_ref() else {
            return Err(RuntimeError::new(
                "runtime model lifetime can only end inside an active action transaction",
            ));
        };

        if transaction.terminated_model_identities.contains(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child is already terminated in this transaction",
            ));
        }
        if !self.dynamic_model_owners.contains_key(identity) {
            return Err(RuntimeError::new(
                "runtime lifetime termination requires an existing committed dynamic child",
            ));
        }
        let actual_owner = self
            .current_dynamic_model_owner(identity)
            .expect("committed live dynamic child should have an owner");
        if actual_owner != claimed_owner {
            return Err(RuntimeError::new(format!(
                "runtime lifetime termination requires rooting owner '{actual_owner}', not '{claimed_owner}'"
            )));
        }

        if self.has_live_dynamic_child_rooted_in(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child roots another live child; cascading lifetime termination is not selected",
            ));
        }

        let designation_deleted_states = self
            .transaction
            .as_ref()
            .expect("transaction should exist")
            .deleted_states
            .clone();
        let mut designation_metadata = self
            .states
            .iter()
            .filter(|(name, _)| !designation_deleted_states.contains(*name))
            .filter_map(|(name, cell)| {
                cell.designation
                    .as_ref()
                    .map(|metadata| (name.clone(), metadata.allows_none))
            })
            .collect::<Vec<_>>();
        designation_metadata.extend(
            self.transaction
                .as_ref()
                .expect("transaction should exist")
                .created_states
                .iter()
                .filter(|(name, _)| !designation_deleted_states.contains(*name))
                .filter_map(|(name, cell)| {
                    cell.designation
                        .as_ref()
                        .map(|metadata| (name.clone(), metadata.allows_none))
                }),
        );
        let mut optional_designations = Vec::new();
        for (name, allows_none) in designation_metadata {
            let value = self.read_global_state(&name, None)?;
            let Value::String(target) = value else {
                return Err(RuntimeError::new(
                    "internal persistent live designation carrier must be String",
                ));
            };
            if target != identity {
                continue;
            }
            if !allows_none {
                return Err(RuntimeError::new(
                    "runtime modeled-state child cannot terminate while a plain live designation still targets it",
                ));
            }
            optional_designations.push(name);
        }

        let member_prefix = format!("__meld_sm${identity}$");
        let deleted_states = self
            .states
            .keys()
            .filter(|name| name.starts_with(&member_prefix))
            .cloned()
            .collect::<Vec<_>>();
        let deleted_derived = self
            .derived
            .keys()
            .filter(|name| name.starts_with(&member_prefix))
            .cloned()
            .collect::<Vec<_>>();

        let staged_deleted_states = self
            .transaction
            .as_ref()
            .expect("transaction should exist")
            .deleted_states
            .clone();
        let mut state_names = self.states.keys().cloned().collect::<HashSet<_>>();
        state_names.extend(
            self.transaction
                .as_ref()
                .expect("transaction should exist")
                .created_states
                .keys()
                .cloned(),
        );
        state_names.retain(|name| {
            !staged_deleted_states.contains(name) && !name.starts_with(&member_prefix)
        });

        let mut membership_updates = Vec::new();
        for name in state_names {
            let value = self.read_global_state(&name, None)?;
            let Value::Sequence {
                element_model,
                targets,
            } = value
            else {
                continue;
            };
            if !targets.iter().any(|target| target == identity) {
                continue;
            }
            let targets = targets
                .into_iter()
                .filter(|target| target != identity)
                .collect();
            membership_updates.push((
                name,
                Value::Sequence {
                    element_model,
                    targets,
                },
            ));
        }

        for name in optional_designations {
            self.write_state(&name, Value::String(String::new()))?;
        }
        for (name, value) in membership_updates {
            self.write_state(&name, value)?;
        }

        for name in &deleted_states {
            self.invalidate_transaction_dependency(name);
        }
        for name in &deleted_derived {
            self.invalidate_transaction_dependency(name);
            self.clear_transaction_dependencies(name);
        }

        let transaction = self
            .transaction
            .as_mut()
            .expect("transaction should exist while ending model lifetime");
        for name in deleted_states {
            transaction.writes.remove(&name);
            transaction.deleted_states.insert(name);
        }
        for name in deleted_derived {
            transaction.derived_values.remove(&name);
            transaction.derived_dependencies.remove(&name);
            transaction.deleted_derived.insert(name);
        }
        transaction
            .terminated_model_identities
            .insert(identity.to_string());

        Ok(())
    }

    fn invoke_transfer_builtin(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(target), ActionArgument::Value(source_owner), ActionArgument::Value(destination_owner)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal child-transfer builtin expects target, source owner, and destination owner",
            ));
        };

        let target = match self.eval_expr(target, None)? {
            Value::String(target) if target.is_empty() => {
                return Err(RuntimeError::new(
                    "transfer requires a present live designation",
                ));
            }
            Value::String(target) => target,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-transfer target must be String, got {}",
                    other.type_name()
                )));
            }
        };
        let source_owner = match self.eval_expr(source_owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "transfer requires a present source owner designation",
                ));
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-transfer source owner must be String, got {}",
                    other.type_name()
                )));
            }
        };
        let destination_owner = match self.eval_expr(destination_owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "transfer requires a present destination owner designation",
                ));
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-transfer destination owner must be String, got {}",
                    other.type_name()
                )));
            }
        };

        self.transfer_runtime_model_owner(&target, &source_owner, &destination_owner)
    }

    fn invoke_destroy_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(target), ActionArgument::Value(owner)] = arguments else {
            return Err(RuntimeError::new(
                "internal child-destroy builtin expects designation and owner",
            ));
        };

        let target = match self.eval_expr(target, None)? {
            Value::String(target) => target,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-destroy designation must be String, got {}",
                    other.type_name()
                )))
            }
        };
        if target.is_empty() {
            return Err(RuntimeError::new(
                "destroy requires a present maybe live designation",
            ));
        }

        let owner = match self.eval_expr(owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "destroy requires a present live owner designation",
                ))
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-destroy owner must be String, got {}",
                    other.type_name()
                )))
            }
        };

        self.terminate_runtime_model(&target, &owner)
    }

    fn invoke_purge_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(target), ActionArgument::Value(owner)] = arguments else {
            return Err(RuntimeError::new(
                "internal subtree-purge builtin expects designation and owner",
            ));
        };

        let target = match self.eval_expr(target, None)? {
            Value::String(target) => target,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal subtree-purge designation must be String, got {}",
                    other.type_name()
                )))
            }
        };
        if target.is_empty() {
            return Err(RuntimeError::new(
                "purge requires a present maybe live designation",
            ));
        }

        let owner = match self.eval_expr(owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "purge requires a present live owner designation",
                ))
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal subtree-purge owner must be String, got {}",
                    other.type_name()
                )))
            }
        };

        self.terminate_runtime_model_subtree(&target, &owner)
    }

    fn invoke_action(
        &mut self,
        name: &str,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        if name == crate::lifetime_transfer_surface::TRANSFER_BUILTIN_ACTION {
            return self.invoke_transfer_builtin(arguments);
        }
        if name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION {
            return self.invoke_destroy_builtin(arguments);
        }
        if name == crate::lifetime_termination_surface::PURGE_BUILTIN_ACTION {
            return self.invoke_purge_builtin(arguments);
        }
        if name == crate::create_surface::CREATE_BUILTIN_ACTION {
            return self.invoke_create_builtin(arguments);
        }
        if name == crate::scoped_create_surface::CREATE_SCOPE_BUILTIN_ACTION {
            return self.invoke_create_scope_builtin(arguments);
        }
        if name == crate::scoped_designation_surface::SCOPE_BUILTIN_ACTION {
            return self.invoke_designation_scope_builtin(arguments);
        }
        if name.starts_with(
            crate::existing_designation_insert_surface::GENERATED_EXISTING_INSERT_ACTION_PREFIX,
        ) {
            return self.invoke_existing_designation_insert(arguments);
        }
        if name.starts_with(crate::scoped_create_surface::GENERATED_INSERT_ACTION_PREFIX) {
            return self.invoke_scoped_insert(arguments);
        }
        if name.starts_with(crate::structural_move_surface::GENERATED_FILTERED_MOVE_ACTION_PREFIX) {
            return self.invoke_filtered_structural_move(arguments);
        }
        if name.starts_with(crate::structural_move_surface::GENERATED_MOVE_ACTION_PREFIX) {
            return self.invoke_structural_move(arguments);
        }
        if name.starts_with(crate::structural_edit_surface::GENERATED_FILTERED_REMOVE_ACTION_PREFIX)
        {
            return self.invoke_filtered_structural_remove(arguments);
        }
        if name.starts_with(crate::structural_edit_surface::GENERATED_REMOVE_ACTION_PREFIX) {
            return self.invoke_structural_remove(arguments);
        }

        if self.action_stack.iter().any(|entry| entry == name) {
            let mut cycle = self.action_stack.join(" -> ");
            if !cycle.is_empty() {
                cycle.push_str(" -> ");
            }
            cycle.push_str(name);
            return Err(RuntimeError::new(format!(
                "recursive action call is not supported in the bootstrap runtime: {cycle}"
            )));
        }

        let action = self
            .actions
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown action '{name}'")))?;

        let frame = self.bind_action_arguments(&action, arguments)?;

        self.action_stack.push(name.to_string());
        self.action_frames.push(frame);

        let result = self.exec_statements(&action.statements);

        self.action_frames.pop();
        self.action_stack.pop();

        result
    }

    fn invoke_create_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(model), ActionArgument::Value(owner)] = arguments else {
            return Err(RuntimeError::new(
                "internal create builtin requires model and owner arguments",
            ));
        };

        let model = match self.eval_expr(model, None)? {
            Value::String(model) => model,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal create model must be String, got {}",
                    other.type_name()
                )))
            }
        };
        let owner = match self.eval_expr(owner, None)? {
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal create owner must be String, got {}",
                    other.type_name()
                )))
            }
        };

        self.instantiate_runtime_model(&model, &owner)?;
        Ok(())
    }

    fn invoke_create_scope_builtin(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(model), ActionArgument::Value(owner), ActionArgument::Value(scope_action)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal scoped create builtin requires model, owner, and scope action arguments",
            ));
        };

        let model = match self.eval_expr(model, None)? {
            Value::String(model) => model,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped create model must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let owner = match self.eval_expr(owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "scoped create requires a present live owner designation",
                ))
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped create owner must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let scope_action = match self.eval_expr(scope_action, None)? {
            Value::String(scope_action) => scope_action,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped create action must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let template = self
            .runtime_model_templates
            .get(&model)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown runtime model '{model}'")))?;

        let identity = self.instantiate_runtime_model(&model, &owner)?;

        let mut arguments = vec![ActionArgument::Value(Expr::String(identity.clone()))];
        arguments.extend(
            template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
                .map(|member| ActionArgument::StateGrant {
                    location: crate::ast::SourceLocation::new(1, 1),
                    name: model_binding_name(&identity, &member.name),
                }),
        );

        self.invoke_action(&scope_action, &arguments)
    }

    fn invoke_designation_scope_builtin(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(model), ActionArgument::Value(designation), ActionArgument::Value(scope_action)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal scoped designation builtin requires model, designation, and scope action arguments",
            ));
        };

        let model = match self.eval_expr(model, None)? {
            Value::String(model) => model,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped designation model must be String, got {}",
                    other.type_name()
                )))
            }
        };
        let identity = match self.eval_expr(designation, None)? {
            Value::String(identity) => identity,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped designation must be String, got {}",
                    other.type_name()
                )))
            }
        };
        if identity.is_empty() {
            return Err(RuntimeError::new("live designation has no target"));
        }
        if !self.model_identity_exists(&identity) {
            return Err(RuntimeError::new(format!(
                "unknown live identity '{identity}'"
            )));
        }
        let scope_action = match self.eval_expr(scope_action, None)? {
            Value::String(scope_action) => scope_action,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped designation action must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let template = self
            .runtime_model_templates
            .get(&model)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown runtime model '{model}'")))?;
        let mut arguments = vec![ActionArgument::Value(Expr::String(identity.clone()))];
        arguments.extend(
            template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
                .map(|member| ActionArgument::StateGrant {
                    location: crate::ast::SourceLocation::new(1, 1),
                    name: model_binding_name(&identity, &member.name),
                }),
        );
        self.invoke_action(&scope_action, &arguments)
    }

    fn invoke_existing_designation_insert(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(identity), ActionArgument::StateGrant { name: target, .. }, ActionArgument::Value(owner)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal existing-designation insertion requires identity, owner, and sequence authority arguments",
            ));
        };

        let identity = match self.eval_expr(identity, None)? {
            Value::String(identity) if !identity.is_empty() => identity,
            Value::String(_) => return Err(RuntimeError::new("live designation has no target")),
            other => {
                return Err(RuntimeError::new(format!(
                    "internal existing-designation insertion identity must be String, got {}",
                    other.type_name()
                )))
            }
        };
        let target_owner = match self.eval_expr(owner, None)? {
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal existing-designation insertion owner must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let actual_owner = self
            .transaction
            .as_ref()
            .and_then(|transaction| transaction.created_model_owners.get(&identity))
            .or_else(|| self.dynamic_model_owners.get(&identity))
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "existing-designation insertion requires an owner-relative dynamic child identity; '{identity}' has no dynamic owner"
                ))
            })?;

        if actual_owner != target_owner {
            return Err(RuntimeError::new(format!(
                "live identity '{identity}' belongs to owner '{actual_owner}' and cannot be inserted into membership of owner '{target_owner}'"
            )));
        }

        self.invoke_scoped_insert(&[
            ActionArgument::Value(Expr::String(identity)),
            ActionArgument::StateGrant {
                location: crate::ast::SourceLocation::new(1, 1),
                name: target.clone(),
            },
        ])
    }

    fn invoke_scoped_insert(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(identity), ActionArgument::StateGrant { name: target, .. }] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal scoped insertion requires identity and sequence authority arguments",
            ));
        };

        let identity = match self.eval_expr(identity, None)? {
            Value::String(identity) => identity,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal scoped insertion identity must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let target = self.resolve_state_grant(target)?;

        let expected_model = match self.state_type(&target) {
            Some(ValueType::SequenceLive(model)) => model,
            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal scoped insertion target must be [live T], got {}",
                    show_type(&other)
                )))
            }
            None => {
                return Err(RuntimeError::new(format!(
                    "unknown scoped insertion target state '{target}'"
                )))
            }
        };

        let current = self.read_global_state(&target, None)?;
        let Value::Sequence {
            element_model,
            mut targets,
        } = current
        else {
            return Err(RuntimeError::new(
                "internal scoped insertion target did not contain a runtime sequence",
            ));
        };

        if element_model != expected_model {
            return Err(RuntimeError::new(format!(
                "internal scoped insertion target carries live {element_model} but expects live {expected_model}"
            )));
        }

        if !self.model_identity_exists(&identity) {
            return Err(RuntimeError::new(format!(
                "unknown live identity '{identity}'"
            )));
        }

        targets.push(identity);
        self.write_state(
            &target,
            Value::Sequence {
                element_model,
                targets,
            },
        )
    }

    fn invoke_filtered_structural_move(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::StateGrant { name: target, .. }, ActionArgument::Value(view), ActionArgument::Value(moving), ActionArgument::Value(anchor), ActionArgument::Value(placement)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal filtered structural move requires backing sequence authority, current view, moving designation, anchor designation, and placement",
            ));
        };

        let view = self.eval_expr(view, None)?;
        self.invoke_structural_move_common(target, Some(view), moving, anchor, placement)
    }

    fn invoke_structural_move(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::StateGrant { name: target, .. }, ActionArgument::Value(moving), ActionArgument::Value(anchor), ActionArgument::Value(placement)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal structural move requires sequence authority, moving designation, anchor designation, and placement",
            ));
        };

        self.invoke_structural_move_common(target, None, moving, anchor, placement)
    }

    fn invoke_structural_move_common(
        &mut self,
        target: &str,
        selection: Option<Value>,
        moving: &Expr,
        anchor: &Expr,
        placement: &Expr,
    ) -> Result<(), RuntimeError> {
        let target = if let Some(binding) = self
            .action_frames
            .last()
            .and_then(|frame| frame.bindings.get(target))
        {
            match binding {
                ActionBinding::State(state_name) => state_name.clone(),
                ActionBinding::Value(_) => {
                    return Err(RuntimeError::new(
                        "internal structural move target is not writable state",
                    ))
                }
            }
        } else {
            target.to_string()
        };

        let expected_model = match self.state_type(&target) {
            Some(ValueType::SequenceLive(model)) => model,
            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal structural move target must be [live T], got {}",
                    show_type(&other)
                )))
            }
            None => {
                return Err(RuntimeError::new(format!(
                    "unknown structural move target state '{target}'"
                )))
            }
        };

        let current = self.read_global_state(&target, None)?;
        let Value::Sequence {
            element_model,
            targets,
        } = current
        else {
            return Err(RuntimeError::new(
                "internal structural move target did not contain a runtime sequence",
            ));
        };

        let (selection_model, selection_targets) = match selection {
            Some(Value::Sequence {
                element_model,
                targets,
            }) => (element_model, targets),
            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal filtered structural move view must be [live T], got {}",
                    other.type_name()
                )))
            }
            None => (element_model.clone(), targets.clone()),
        };

        if element_model != expected_model || selection_model != expected_model {
            return Err(RuntimeError::new(format!(
                "structural move model mismatch: backing live {element_model}, selection live {selection_model}, expected live {expected_model}"
            )));
        }

        let moving = match self.eval_expr(moving, None)? {
            Value::String(identity) if identity.is_empty() => {
                return Err(RuntimeError::new("moving live designation has no target"))
            }
            Value::String(identity) => identity,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal structural move moving designation must be live identity, got {}",
                    other.type_name()
                )))
            }
        };
        let anchor = match self.eval_expr(anchor, None)? {
            Value::String(identity) if identity.is_empty() => {
                return Err(RuntimeError::new("anchor live designation has no target"))
            }
            Value::String(identity) => identity,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal structural move anchor designation must be live identity, got {}",
                    other.type_name()
                )))
            }
        };
        let placement = match self.eval_expr(placement, None)? {
            Value::String(value) if value == "before" => RelativePlacement::Before,
            Value::String(value) if value == "after" => RelativePlacement::After,
            Value::String(value) => {
                return Err(RuntimeError::new(format!(
                    "internal structural move placement '{value}' is invalid"
                )))
            }
            other => {
                return Err(RuntimeError::new(format!(
                    "internal structural move placement must be String, got {}",
                    other.type_name()
                )))
            }
        };

        let reordered = move_unique_relative(
            &targets,
            &selection_targets,
            &moving,
            &anchor,
            placement,
        )
        .map_err(|error| match error {
            StructuralMoveError::MovingNoCurrentOccurrence => RuntimeError::new(format!(
                "moving live designation '{moving}' has no current occurrence in structural move selection"
            )),
            StructuralMoveError::MovingAmbiguousCurrentOccurrence => RuntimeError::new(format!(
                "moving live designation '{moving}' has multiple current occurrences in structural move selection"
            )),
            StructuralMoveError::AnchorNoCurrentOccurrence => RuntimeError::new(format!(
                "anchor live designation '{anchor}' has no current occurrence in structural move selection"
            )),
            StructuralMoveError::AnchorAmbiguousCurrentOccurrence => RuntimeError::new(format!(
                "anchor live designation '{anchor}' has multiple current occurrences in structural move selection"
            )),
            StructuralMoveError::ViewDoesNotMapToBacking => RuntimeError::new(
                "structural move selection could not be mapped to current backing membership",
            ),
        })?;

        self.write_state(
            &target,
            Value::Sequence {
                element_model,
                targets: reordered,
            },
        )
    }

    fn invoke_filtered_structural_remove(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::StateGrant { name: target, .. }, ActionArgument::Value(view), ActionArgument::Value(index)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal filtered structural removal requires backing sequence authority, current view, and index arguments",
            ));
        };

        let view = self.eval_expr(view, None)?;
        let Value::Sequence {
            element_model: view_model,
            targets: view_targets,
        } = view
        else {
            return Err(RuntimeError::new(
                "internal filtered structural removal view did not evaluate to an ordered sequence",
            ));
        };

        let selector = self.eval_expr(index, None)?;
        let index = match selector {
            Value::Int(index) if index >= 0 => index as usize,
            Value::Int(_) => {
                return Err(RuntimeError::new(
                    "filtered structural removal index cannot be negative",
                ))
            }
            Value::String(identity) => {
                if identity.is_empty() {
                    return Err(RuntimeError::new("live designation has no target"));
                }
                match resolve_unique_occurrence(&identity, &view_targets) {
                    Ok(index) => index,
                    Err(UniqueOccurrenceError::NoCurrentOccurrence) => {
                        return Err(RuntimeError::new(
                            "selected live designation has no current occurrence in filtered structural removal view",
                        ))
                    }
                    Err(UniqueOccurrenceError::AmbiguousCurrentOccurrence) => {
                        return Err(RuntimeError::new(
                            "selected live designation has multiple current occurrences in filtered structural removal view",
                        ))
                    }
                }
            }
            other => {
                return Err(RuntimeError::new(format!(
                    "internal filtered structural removal selector must be Int or live designation, got {}",
                    other.type_name()
                )))
            }
        };

        if index >= view_targets.len() {
            return Err(RuntimeError::new(format!(
                "filtered structural removal index {index} is out of bounds for view of length {}",
                view_targets.len()
            )));
        }

        let target =
            if let Some(binding) = self
                .action_frames
                .last()
                .and_then(|frame| frame.bindings.get(target))
            {
                match binding {
                    ActionBinding::State(state_name) => state_name.clone(),
                    ActionBinding::Value(_) => return Err(RuntimeError::new(
                        "internal filtered structural removal backing target is not writable state",
                    )),
                }
            } else {
                target.clone()
            };

        let expected_model = match self.state_type(&target) {
            Some(ValueType::SequenceLive(model)) => model,
            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal filtered structural removal backing target must be [live T], got {}",
                    show_type(&other)
                )))
            }
            None => {
                return Err(RuntimeError::new(format!(
                    "unknown filtered structural removal backing state '{target}'"
                )))
            }
        };

        let current = self.read_global_state(&target, None)?;
        let Value::Sequence {
            element_model,
            mut targets,
        } = current
        else {
            return Err(RuntimeError::new(
                "internal filtered structural removal backing target did not contain a runtime sequence",
            ));
        };

        if element_model != expected_model || view_model != expected_model {
            return Err(RuntimeError::new(format!(
                "filtered structural removal model mismatch: backing live {element_model}, view live {view_model}, expected live {expected_model}"
            )));
        }

        let mut view_cursor = 0usize;
        let mut backing_index = None;
        for (source_index, source_target) in targets.iter().enumerate() {
            if view_cursor < view_targets.len() && source_target == &view_targets[view_cursor] {
                if view_cursor == index {
                    backing_index = Some(source_index);
                    break;
                }
                view_cursor += 1;
            }
        }

        let Some(backing_index) = backing_index else {
            return Err(RuntimeError::new(
                "internal filtered structural removal could not map the selected view occurrence to current backing membership",
            ));
        };

        targets.remove(backing_index);
        self.write_state(
            &target,
            Value::Sequence {
                element_model,
                targets,
            },
        )
    }

    fn invoke_structural_remove(
        &mut self,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        let [ActionArgument::StateGrant { name: target, .. }, ActionArgument::Value(index)] =
            arguments
        else {
            return Err(RuntimeError::new(
                "internal structural removal requires sequence authority and index arguments",
            ));
        };

        let selector = self.eval_expr(index, None)?;

        let target = self.resolve_state_grant(target)?;

        let expected_model = match self.state_type(&target) {
            Some(ValueType::SequenceLive(model)) => model,
            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal structural removal target must be [live T], got {}",
                    show_type(&other)
                )))
            }
            None => {
                return Err(RuntimeError::new(format!(
                    "unknown structural removal target state '{target}'"
                )))
            }
        };

        let current = self.read_global_state(&target, None)?;
        let Value::Sequence {
            element_model,
            mut targets,
        } = current
        else {
            return Err(RuntimeError::new(
                "internal structural removal target did not contain a runtime sequence",
            ));
        };

        if element_model != expected_model {
            return Err(RuntimeError::new(format!(
                "internal structural removal target carries live {element_model} but expects live {expected_model}"
            )));
        }

        let index = match selector {
            Value::Int(index) if index >= 0 => index as usize,
            Value::Int(_) => {
                return Err(RuntimeError::new(
                    "structural removal index cannot be negative",
                ))
            }
            Value::String(identity) => {
                if identity.is_empty() {
                    return Err(RuntimeError::new("live designation has no target"));
                }
                match resolve_unique_occurrence(&identity, &targets) {
                    Ok(index) => index,
                    Err(UniqueOccurrenceError::NoCurrentOccurrence) => {
                        return Err(RuntimeError::new(
                            "selected live designation has no current occurrence in structural removal target",
                        ))
                    }
                    Err(UniqueOccurrenceError::AmbiguousCurrentOccurrence) => {
                        return Err(RuntimeError::new(
                            "selected live designation has multiple current occurrences in structural removal target",
                        ))
                    }
                }
            }
            other => {
                return Err(RuntimeError::new(format!(
                    "internal structural removal selector must be Int or live designation, got {}",
                    other.type_name()
                )))
            }
        };

        if index >= targets.len() {
            return Err(RuntimeError::new(format!(
                "structural removal index {index} is out of bounds for sequence of length {}",
                targets.len()
            )));
        }

        targets.remove(index);
        self.write_state(
            &target,
            Value::Sequence {
                element_model,
                targets,
            },
        )
    }

    fn bind_action_arguments(
        &mut self,
        action: &ActionDecl,
        arguments: &[ActionArgument],
    ) -> Result<ActionFrame, RuntimeError> {
        if action.parameters.len() != arguments.len() {
            return Err(RuntimeError::new(format!(
                "action '{}' expects {} arguments but got {}",
                action.name,
                action.parameters.len(),
                arguments.len()
            )));
        }

        let parameter_types = self
            .action_parameter_types
            .get(&action.name)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "checked parameter type information missing for action '{}'",
                    action.name
                ))
            })?;

        if parameter_types.len() != action.parameters.len() {
            return Err(RuntimeError::new(format!(
                "checked parameter type information for action '{}' has {} entries but the action declares {} parameters",
                action.name,
                parameter_types.len(),
                action.parameters.len()
            )));
        }

        let mut frame = ActionFrame::default();

        for ((parameter, argument), expected_type) in
            action.parameters.iter().zip(arguments).zip(parameter_types)
        {
            let binding = match (parameter.kind, argument) {
                (ActionParameterKind::Value, ActionArgument::Value(expression)) => {
                    let value = self.eval_expr(expression, None)?;
                    let value = coerce_value(value, &expected_type)?;
                    ActionBinding::Value(value)
                }
                (ActionParameterKind::State, ActionArgument::StateGrant { name, .. }) => {
                    let state_name = self.resolve_state_grant(name)?;
                    let actual_type = self.state_type(&state_name).ok_or_else(|| {
                        RuntimeError::new(format!(
                            "unknown state '{}' while binding action parameter '{}'",
                            state_name, parameter.name
                        ))
                    })?;

                    if actual_type != expected_type {
                        return Err(RuntimeError::new(format!(
                            "writable state '{}' has type {} but parameter '{}' requires {}",
                            state_name,
                            show_type(&actual_type),
                            parameter.name,
                            show_type(&expected_type)
                        )));
                    }

                    ActionBinding::State(state_name)
                }
                (ActionParameterKind::Value, ActionArgument::StateGrant { name, .. }) => {
                    return Err(RuntimeError::new(format!(
                        "value parameter '{}' cannot receive writable state grant 'state {}'",
                        parameter.name, name
                    )));
                }
                (_, ActionArgument::IndexedStateGrant { .. }) => {
                    return Err(RuntimeError::new(
                        "internal source indexed state grant reached runtime before grant transport",
                    ));
                }
                (ActionParameterKind::State, ActionArgument::Value(_)) => {
                    return Err(RuntimeError::new(format!(
                        "state parameter '{}' requires explicit writable state authority",
                        parameter.name
                    )));
                }
            };

            frame.bindings.insert(parameter.name.clone(), binding);
        }

        Ok(frame)
    }

    fn resolve_state_grant(&mut self, name: &str) -> Result<String, RuntimeError> {
        if let Some(expression) = self.runtime_index_grant_carriers.get(name).cloned() {
            return match expression {
                Expr::RuntimeIndexMember {
                    source,
                    index,
                    member,
                    element_model,
                    ..
                } => {
                    let target = self.runtime_index_target(
                        source.as_ref(),
                        index.as_ref(),
                        &element_model,
                        None,
                    )?;
                    Ok(model_binding_name(&target, &member))
                }
                Expr::RuntimeDesignationMember {
                    designation,
                    member,
                    ..
                } => {
                    let target = match self.eval_expr(designation.as_ref(), None)? {
                        Value::String(target) if target.is_empty() => {
                            return Err(RuntimeError::new("live designation has no target"));
                        }
                        Value::String(target) => target,
                        other => {
                            return Err(RuntimeError::new(format!(
                                "runtime designation grant must resolve to live identity, got {}",
                                other.type_name()
                            )));
                        }
                    };
                    if !self.model_identity_exists(&target) {
                        return Err(RuntimeError::new(format!(
                            "unknown live identity '{target}'"
                        )));
                    }
                    Ok(model_binding_name(&target, &member))
                }
                _ => Err(RuntimeError::new(format!(
                    "runtime indexed grant carrier '{name}' is malformed"
                ))),
            };
        }

        if let Some(binding) = self
            .action_frames
            .last()
            .and_then(|frame| frame.bindings.get(name))
        {
            return match binding {
                ActionBinding::State(state_name) => Ok(state_name.clone()),
                ActionBinding::Value(_) => Err(RuntimeError::new(format!(
                    "value parameter '{}' does not carry writable state authority",
                    name
                ))),
            };
        }

        if self.state_exists(name) {
            return Ok(name.to_string());
        }

        if self.derived_exists(name) {
            return Err(RuntimeError::new(format!(
                "derived value '{}' cannot be granted as writable state",
                name
            )));
        }

        Err(RuntimeError::new(format!(
            "unknown writable state '{}'",
            name
        )))
    }

    fn exec_statements(&mut self, statements: &[Statement]) -> Result<(), RuntimeError> {
        for statement in statements {
            self.exec_statement(statement)?;
        }
        Ok(())
    }

    fn exec_statement(&mut self, statement: &Statement) -> Result<(), RuntimeError> {
        match statement {
            Statement::Assignment {
                target,
                operator,
                value,
                ..
            } => {
                let rhs = self.eval_expr(value, None)?;
                let new_value = match operator {
                    AssignmentOperator::Assign => rhs,
                    AssignmentOperator::AddAssign => {
                        let left = self.read_name(target, None)?;
                        apply_binary(BinaryOperator::Add, left, rhs)?
                    }
                    AssignmentOperator::SubtractAssign => {
                        let left = self.read_name(target, None)?;
                        apply_binary(BinaryOperator::Subtract, left, rhs)?
                    }
                    AssignmentOperator::MultiplyAssign => {
                        let left = self.read_name(target, None)?;
                        apply_binary(BinaryOperator::Multiply, left, rhs)?
                    }
                    AssignmentOperator::DivideAssign => {
                        let left = self.read_name(target, None)?;
                        apply_binary(BinaryOperator::Divide, left, rhs)?
                    }
                };
                self.write_state(target, new_value)
            }
            Statement::IndexedThroughAssignment { .. } => Err(RuntimeError::new(
                "internal source indexed assignment reached runtime before sequence lowering",
            )),
            Statement::RuntimeIndexAssignment {
                source,
                index,
                member,
                element_model,
                operator,
                value,
                ..
            } => {
                let source = Expr::Name(source.clone());
                let target =
                    self.runtime_index_target(&source, index.as_ref(), element_model, None)?;
                let member_name = model_binding_name(&target, member);
                let rhs = self.eval_expr(value, None)?;
                let new_value = match operator {
                    AssignmentOperator::Assign => rhs,
                    AssignmentOperator::AddAssign => {
                        let left = self.read_name(&member_name, None)?;
                        apply_binary(BinaryOperator::Add, left, rhs)?
                    }
                    AssignmentOperator::SubtractAssign => {
                        let left = self.read_name(&member_name, None)?;
                        apply_binary(BinaryOperator::Subtract, left, rhs)?
                    }
                    AssignmentOperator::MultiplyAssign => {
                        let left = self.read_name(&member_name, None)?;
                        apply_binary(BinaryOperator::Multiply, left, rhs)?
                    }
                    AssignmentOperator::DivideAssign => {
                        let left = self.read_name(&member_name, None)?;
                        apply_binary(BinaryOperator::Divide, left, rhs)?
                    }
                };
                self.write_state(&member_name, new_value)
            }
            Statement::RuntimeDesignationAssignment {
                designation,
                member,
                operator,
                value,
                ..
            } => {
                let target = match self.eval_expr(designation, None)? {
                    Value::String(target) => target,
                    other => {
                        return Err(RuntimeError::new(format!(
                            "internal live designation carrier must be String, got {}",
                            other.type_name()
                        )))
                    }
                };
                if target.is_empty() {
                    return Err(RuntimeError::new("live designation has no target"));
                }
                let state_name = model_binding_name(&target, member);
                let right = self.eval_expr(value, None)?;
                let next = match operator {
                    AssignmentOperator::Assign => right,
                    AssignmentOperator::AddAssign => apply_binary(
                        BinaryOperator::Add,
                        self.read_name(&state_name, None)?,
                        right,
                    )?,
                    AssignmentOperator::SubtractAssign => apply_binary(
                        BinaryOperator::Subtract,
                        self.read_name(&state_name, None)?,
                        right,
                    )?,
                    AssignmentOperator::MultiplyAssign => apply_binary(
                        BinaryOperator::Multiply,
                        self.read_name(&state_name, None)?,
                        right,
                    )?,
                    AssignmentOperator::DivideAssign => apply_binary(
                        BinaryOperator::Divide,
                        self.read_name(&state_name, None)?,
                        right,
                    )?,
                };
                self.write_state(&state_name, next)
            }
            Statement::ActionCall {
                name, arguments, ..
            } => self.invoke_action(name, arguments),
            Statement::Fail { message, .. } => match self.eval_expr(message, None)? {
                Value::String(message) => {
                    Err(RuntimeError::new(format!("action failed: {message}")))
                }
                other => Err(RuntimeError::new(format!(
                    "fail message must evaluate to String, got {}",
                    other.type_name()
                ))),
            },
            Statement::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => match self.eval_expr(condition, None)? {
                Value::Bool(true) => self.exec_statements(then_branch),
                Value::Bool(false) => match else_branch {
                    Some(else_branch) => self.exec_statements(else_branch),
                    None => Ok(()),
                },
                other => Err(RuntimeError::new(format!(
                    "if statement condition must evaluate to Bool, got {}",
                    other.type_name()
                ))),
            },
        }
    }

    fn eval_expr(&mut self, expr: &Expr, owner: Option<&str>) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Integer(value) => Ok(Value::Int(*value)),
            Expr::Float(value) => Ok(Value::Float(*value)),
            Expr::Bool(value) => Ok(Value::Bool(*value)),
            Expr::String(value) => {
                if let Some((element_model, targets)) = decode_runtime_sequence_value(value) {
                    Ok(Value::Sequence {
                        element_model,
                        targets,
                    })
                } else {
                    Ok(Value::String(value.clone()))
                }
            }
            Expr::Name(name) => self.read_name(name, owner),
            Expr::Binary {
                operator,
                left,
                right,
            } => match operator {
                BinaryOperator::And => match self.eval_expr(left, owner)? {
                    Value::Bool(false) => Ok(Value::Bool(false)),
                    Value::Bool(true) => match self.eval_expr(right, owner)? {
                        Value::Bool(value) => Ok(Value::Bool(value)),
                        other => Err(RuntimeError::new(format!(
                            "right operand of and must evaluate to Bool, got {}",
                            other.type_name()
                        ))),
                    },
                    other => Err(RuntimeError::new(format!(
                        "left operand of and must evaluate to Bool, got {}",
                        other.type_name()
                    ))),
                },
                BinaryOperator::Or => match self.eval_expr(left, owner)? {
                    Value::Bool(true) => Ok(Value::Bool(true)),
                    Value::Bool(false) => match self.eval_expr(right, owner)? {
                        Value::Bool(value) => Ok(Value::Bool(value)),
                        other => Err(RuntimeError::new(format!(
                            "right operand of or must evaluate to Bool, got {}",
                            other.type_name()
                        ))),
                    },
                    other => Err(RuntimeError::new(format!(
                        "left operand of or must evaluate to Bool, got {}",
                        other.type_name()
                    ))),
                },
                _ => {
                    let left = self.eval_expr(left, owner)?;
                    let right = self.eval_expr(right, owner)?;
                    apply_binary(*operator, left, right)
                }
            },
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => match self.eval_expr(condition, owner)? {
                Value::Bool(true) => self.eval_expr(then_branch, owner),
                Value::Bool(false) => self.eval_expr(else_branch, owner),
                other => Err(RuntimeError::new(format!(
                    "if condition must evaluate to Bool, got {}",
                    other.type_name()
                ))),
            },
            Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => Err(RuntimeError::new(
                "internal source indexed expression reached runtime before sequence lowering",
            )),
            Expr::RuntimeIndexDesignation {
                source,
                index,
                element_model,
            } => Ok(Value::String(self.runtime_index_target(
                source.as_ref(),
                index,
                element_model,
                owner,
            )?)),
            Expr::RuntimeDesignationMember {
                designation,
                member,
                ..
            } => {
                let target = match self.eval_expr(designation, owner)? {
                    Value::String(target) => target,
                    other => {
                        return Err(RuntimeError::new(format!(
                            "internal live designation carrier must be String, got {}",
                            other.type_name()
                        )))
                    }
                };
                if target.is_empty() {
                    return Err(RuntimeError::new("live designation has no target"));
                }
                self.read_name(&model_binding_name(&target, member), owner)
            }
            Expr::RuntimeIndexMember {
                source,
                index,
                member,
                element_model,
                ..
            } => {
                let target = self.runtime_index_target(
                    source.as_ref(),
                    index.as_ref(),
                    element_model,
                    owner,
                )?;
                self.read_name(&model_binding_name(&target, member), owner)
            }
            Expr::Filter {
                source,
                element,
                predicate,
                order_by,
                order_descending,
                element_model,
            } => self.eval_filter(
                source,
                element,
                predicate,
                order_by.as_deref(),
                *order_descending,
                element_model.as_deref(),
                owner,
            ),
        }
    }

    fn runtime_index_target(
        &mut self,
        source: &Expr,
        index: &Expr,
        expected_element_model: &str,
        owner: Option<&str>,
    ) -> Result<String, RuntimeError> {
        let index = match self.eval_expr(index, owner)? {
            Value::Int(index) if index >= 0 => index as usize,
            Value::Int(index) => {
                return Err(RuntimeError::new(format!(
                    "runtime sequence index {index} cannot be negative"
                )))
            }
            other => {
                return Err(RuntimeError::new(format!(
                    "runtime sequence index must evaluate to Int, got {}",
                    other.type_name()
                )))
            }
        };

        let sequence = self.eval_expr(source, owner)?;
        let Value::Sequence {
            element_model,
            targets,
        } = sequence
        else {
            return Err(RuntimeError::new(format!(
                "runtime indexed source must be an ordered sequence, got {}",
                sequence.type_name()
            )));
        };

        if element_model != expected_element_model {
            return Err(RuntimeError::new(format!(
                "runtime indexed source contains live {element_model} but expects live {expected_element_model}"
            )));
        }

        targets.get(index).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "runtime sequence index {index} is out of bounds for sequence of length {}",
                targets.len()
            ))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_filter(
        &mut self,
        source: &Expr,
        element: &str,
        predicate: &Expr,
        order_by: Option<&Expr>,
        order_descending: bool,
        expected_element_model: Option<&str>,
        owner: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        let expected_element_model = expected_element_model.ok_or_else(|| {
            RuntimeError::new("internal runtime filter is missing its live element model")
        })?;

        let sequence = self.eval_expr(source, owner)?;
        let (element_model, targets) = match sequence {
            Value::Sequence {
                element_model,
                targets,
            } => (element_model, targets),
            other => {
                return Err(RuntimeError::new(format!(
                    "runtime filter source must be an ordered sequence, got {}",
                    other.type_name()
                )))
            }
        };

        if element_model != expected_element_model {
            return Err(RuntimeError::new(format!(
                "runtime filter source contains live {} but filter expects live {}",
                element_model, expected_element_model
            )));
        }

        let mut retained = Vec::new();
        for (source_index, target) in targets.into_iter().enumerate() {
            self.element_frames.push(ElementFrame {
                element_name: element.to_string(),
                target: target.clone(),
            });
            let predicate_result = self.eval_expr(predicate, owner);
            let order_result = if matches!(predicate_result, Ok(Value::Bool(true))) {
                order_by.map(|order_by| self.eval_expr(order_by, owner))
            } else {
                None
            };
            let frame = self
                .element_frames
                .pop()
                .expect("runtime filter element frame should exist");

            match predicate_result {
                Ok(Value::Bool(true)) => {
                    let key = match order_result {
                        Some(Ok(Value::Int(value))) => Some(value),
                        Some(Ok(other)) => {
                            return Err(RuntimeError::new(format!(
                                "runtime ordering key for target '{}' must evaluate to Int, got {}",
                                frame.target,
                                other.type_name()
                            )))
                        }
                        Some(Err(error)) => {
                            return Err(RuntimeError::new(format!(
                                "runtime ordering key failed for target '{}': {}",
                                frame.target, error.message
                            )))
                        }
                        None => None,
                    };
                    retained.push((target, key, source_index));
                }
                Ok(Value::Bool(false)) => {}
                Ok(other) => {
                    return Err(RuntimeError::new(format!(
                        "runtime filter predicate for target '{}' must evaluate to Bool, got {}",
                        frame.target,
                        other.type_name()
                    )))
                }
                Err(error) => {
                    return Err(RuntimeError::new(format!(
                        "runtime filter predicate failed for target '{}': {}",
                        frame.target, error.message
                    )))
                }
            }
        }

        if order_by.is_some() {
            retained.sort_by(|left, right| {
                let left_key = left
                    .1
                    .expect("ordered retained element should have an ordering key");
                let right_key = right
                    .1
                    .expect("ordered retained element should have an ordering key");
                let key_order = if order_descending {
                    right_key.cmp(&left_key)
                } else {
                    left_key.cmp(&right_key)
                };
                key_order.then_with(|| left.2.cmp(&right.2))
            });
        }

        Ok(Value::Sequence {
            element_model,
            targets: retained.into_iter().map(|(target, _, _)| target).collect(),
        })
    }

    fn read_name(&mut self, name: &str, owner: Option<&str>) -> Result<Value, RuntimeError> {
        if let Some(frame) = self.reduction_frames.last().cloned() {
            if name == frame.accumulator_name {
                return Ok(frame.accumulator);
            }
        }

        if let Some(frame) = self.element_frames.last().cloned() {
            if name == frame.element_name {
                return Err(RuntimeError::new(
                    "runtime traversal element designation cannot be projected as an ordinary whole value",
                ));
            }
            let prefix = format!("{}.", frame.element_name);
            if let Some(member) = name.strip_prefix(&prefix) {
                if member.is_empty() || member.contains('.') {
                    return Err(RuntimeError::new(
                        "runtime traversal element read must select exactly one modeled-state member",
                    ));
                }
                let lowered = model_binding_name(&frame.target, member);
                return self.read_name(&lowered, owner);
            }
        }

        if let Some(frame) = self.model_frames.last().cloned() {
            if frame.members.contains(name) {
                let rooted = model_binding_name(&frame.root, name);
                return self.read_name(&rooted, owner);
            }
        }

        if owner.is_none() {
            if let Some(binding) = self
                .action_frames
                .last()
                .and_then(|frame| frame.bindings.get(name))
                .cloned()
            {
                return match binding {
                    ActionBinding::Value(value) => Ok(value),
                    ActionBinding::State(state_name) => self.read_global_state(&state_name, owner),
                };
            }
        }

        if self.state_exists(name) {
            return self.read_global_state(name, owner);
        }

        if self.derived_exists(name) {
            if owner != Some(name) {
                self.register_dependency(owner, name);
            }
            return self.eval_derived(name);
        }

        Err(RuntimeError::new(format!("unknown value '{name}'")))
    }

    fn state_exists(&self, name: &str) -> bool {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.deleted_states.contains(name))
        {
            return false;
        }
        self.states.contains_key(name)
            || self
                .transaction
                .as_ref()
                .is_some_and(|transaction| transaction.created_states.contains_key(name))
    }

    fn derived_exists(&self, name: &str) -> bool {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.deleted_derived.contains(name))
        {
            return false;
        }
        self.derived.contains_key(name)
            || self
                .transaction
                .as_ref()
                .is_some_and(|transaction| transaction.created_derived.contains_key(name))
    }

    fn state_type(&self, name: &str) -> Option<ValueType> {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.deleted_states.contains(name))
        {
            return None;
        }
        self.transaction
            .as_ref()
            .and_then(|transaction| transaction.created_states.get(name))
            .or_else(|| self.states.get(name))
            .map(|cell| cell.value_type.clone())
    }

    fn read_global_state(
        &mut self,
        name: &str,
        owner: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        self.register_dependency(owner, name);

        if let Some(value) = self
            .transaction
            .as_ref()
            .and_then(|transaction| transaction.writes.get(name))
        {
            return Ok(value.clone());
        }

        if let Some(value) = self
            .transaction
            .as_ref()
            .and_then(|transaction| transaction.created_states.get(name))
            .map(|cell| cell.value.clone())
        {
            return Ok(value);
        }

        self.states
            .get(name)
            .map(|cell| cell.value.clone())
            .ok_or_else(|| RuntimeError::new(format!("unknown state '{name}'")))
    }

    fn write_state(&mut self, name: &str, value: Value) -> Result<(), RuntimeError> {
        let state_name = if let Some(binding) = self
            .action_frames
            .last()
            .and_then(|frame| frame.bindings.get(name))
        {
            match binding {
                ActionBinding::State(state_name) => state_name.clone(),
                ActionBinding::Value(_) => {
                    return Err(RuntimeError::new(format!(
                        "value parameter '{}' cannot be mutated",
                        name
                    )));
                }
            }
        } else {
            name.to_string()
        };

        let value_type = self
            .state_type(&state_name)
            .ok_or_else(|| RuntimeError::new(format!("unknown state '{state_name}'")))?;

        let value = coerce_value(value, &value_type)?;

        let transaction = self.transaction.as_mut().ok_or_else(|| {
            RuntimeError::new(format!(
                "state '{state_name}' can only be mutated inside an action"
            ))
        })?;

        transaction.writes.insert(state_name.clone(), value);
        self.invalidate_transaction_dependency(&state_name);
        Ok(())
    }

    fn eval_derived(&mut self, name: &str) -> Result<Value, RuntimeError> {
        if let Some(value) = self
            .transaction
            .as_ref()
            .and_then(|transaction| transaction.derived_values.get(name))
        {
            return Ok(value.clone());
        }

        if self.transaction.is_none() {
            if let Some(value) = self.derived.get(name).and_then(|cell| cell.cached.as_ref()) {
                return Ok(value.clone());
            }
        }

        if self.eval_stack.iter().any(|entry| entry == name) {
            let mut cycle = self.eval_stack.join(" -> ");
            if !cycle.is_empty() {
                cycle.push_str(" -> ");
            }
            cycle.push_str(name);
            return Err(RuntimeError::new(format!("cyclic derived state: {cycle}")));
        }

        if self.transaction.is_some() {
            self.clear_transaction_dependencies(name);
        } else {
            self.clear_committed_dependencies(name);
        }

        let cell = self
            .transaction
            .as_ref()
            .and_then(|transaction| transaction.created_derived.get(name))
            .or_else(|| self.derived.get(name))
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown derived value '{name}'")))?;

        let reduction = self.reductions.get(name).cloned();
        self.eval_stack.push(name.to_string());
        if let Some(context) = cell.model_context.clone() {
            self.model_frames.push(context);
        }
        let result = match reduction {
            Some(reduction) => self.eval_runtime_reduction(name, &reduction),
            None => self.eval_expr(&cell.expression, Some(name)),
        };
        if cell.model_context.is_some() {
            self.model_frames
                .pop()
                .expect("runtime model derived frame should exist");
        }
        self.eval_stack.pop();
        let value = coerce_value(result?, &cell.value_type)?;

        if let Some(transaction) = self.transaction.as_mut() {
            if let Some(cell) = transaction.created_derived.get_mut(name) {
                cell.evaluations += 1;
            } else if let Some(cell) = self.derived.get_mut(name) {
                cell.evaluations += 1;
            }
            transaction
                .derived_values
                .insert(name.to_string(), value.clone());
        } else if let Some(cell) = self.derived.get_mut(name) {
            cell.evaluations += 1;
            cell.cached = Some(value.clone());
        }

        Ok(value)
    }

    fn eval_runtime_reduction(
        &mut self,
        owner: &str,
        reduction: &RuntimeReduction,
    ) -> Result<Value, RuntimeError> {
        let sequence = self.read_name(&reduction.spec.source, Some(owner))?;
        let (element_model, targets) = match sequence {
            Value::Sequence {
                element_model,
                targets,
            } => (element_model, targets),
            other => {
                return Err(RuntimeError::new(format!(
                    "runtime reduction source '{}' must be an ordered sequence, got {}",
                    reduction.spec.source,
                    other.type_name()
                )))
            }
        };

        if reduction.spec.element_model.as_deref() != Some(element_model.as_str()) {
            return Err(RuntimeError::new(format!(
                "runtime reduction source '{}' contains live {} but reducer expects live {}",
                reduction.spec.source,
                element_model,
                reduction
                    .spec
                    .element_model
                    .as_deref()
                    .unwrap_or("<unknown>")
            )));
        }

        let initial = self.eval_expr(&reduction.initial, Some(owner))?;
        let mut accumulator = coerce_value(initial, &reduction.result_type).map_err(|error| {
            RuntimeError::new(format!(
                "runtime reduction initial value cannot be stored in {} accumulator: {}",
                show_type(&reduction.result_type),
                error.message
            ))
        })?;

        for target in targets {
            self.reduction_frames.push(ReductionFrame {
                accumulator_name: reduction.spec.accumulator.clone(),
                accumulator,
            });
            self.element_frames.push(ElementFrame {
                element_name: reduction.spec.element.clone(),
                target: target.clone(),
            });
            let result = self.eval_expr(&reduction.step, Some(owner));
            let element_frame = self
                .element_frames
                .pop()
                .expect("runtime reduction element frame should exist");
            self.reduction_frames
                .pop()
                .expect("runtime reduction frame should exist");
            let result = result.map_err(|error| {
                RuntimeError::new(format!(
                    "runtime reduction step failed for target '{}': {}",
                    element_frame.target, error.message
                ))
            })?;
            accumulator = coerce_value(result, &reduction.result_type).map_err(|error| {
                RuntimeError::new(format!(
                    "runtime reduction step for target '{}' cannot be stored in {} accumulator: {}",
                    element_frame.target,
                    show_type(&reduction.result_type),
                    error.message
                ))
            })?;
        }
        Ok(accumulator)
    }

    fn register_dependency(&mut self, owner: Option<&str>, dependency: &str) {
        let Some(owner) = owner else {
            return;
        };

        if let Some(transaction) = self.transaction.as_mut() {
            transaction
                .derived_dependencies
                .entry(owner.to_string())
                .or_default()
                .insert(dependency.to_string());
            transaction
                .dependents
                .entry(dependency.to_string())
                .or_default()
                .insert(owner.to_string());
            return;
        }

        if let Some(owner_cell) = self.derived.get_mut(owner) {
            owner_cell.dependencies.insert(dependency.to_string());
        }

        if let Some(state) = self.states.get_mut(dependency) {
            state.dependents.insert(owner.to_string());
        } else if let Some(derived) = self.derived.get_mut(dependency) {
            derived.dependents.insert(owner.to_string());
        }
    }

    fn clear_transaction_dependencies(&mut self, name: &str) {
        let Some(transaction) = self.transaction.as_mut() else {
            return;
        };

        let dependencies = transaction
            .derived_dependencies
            .remove(name)
            .unwrap_or_default();
        for dependency in dependencies {
            let remove_entry = match transaction.dependents.get_mut(&dependency) {
                Some(watchers) => {
                    watchers.remove(name);
                    watchers.is_empty()
                }
                None => false,
            };
            if remove_entry {
                transaction.dependents.remove(&dependency);
            }
        }
    }

    fn clear_committed_dependencies(&mut self, name: &str) {
        let dependencies = match self.derived.get_mut(name) {
            Some(cell) => std::mem::take(&mut cell.dependencies),
            None => return,
        };

        for dependency in dependencies {
            if let Some(state) = self.states.get_mut(&dependency) {
                state.dependents.remove(name);
            } else if let Some(derived) = self.derived.get_mut(&dependency) {
                derived.dependents.remove(name);
            }
        }
    }

    fn invalidate_transaction_dependency(&mut self, dependency: &str) {
        let Some(transaction) = self.transaction.as_mut() else {
            return;
        };

        let mut pending = vec![dependency.to_string()];
        let mut visited = HashSet::new();

        while let Some(dependency) = pending.pop() {
            if !visited.insert(dependency.clone()) {
                continue;
            }
            let watchers = transaction
                .dependents
                .get(&dependency)
                .cloned()
                .unwrap_or_default();
            for watcher in watchers {
                transaction.derived_values.remove(&watcher);
                pending.push(watcher);
            }
        }
    }

    fn commit(&mut self, transaction: Transaction) {
        let Transaction {
            writes,
            created_states,
            created_derived,
            created_model_owners,
            created_model_types,
            updated_model_owners,
            deleted_states,
            deleted_derived,
            terminated_model_identities,
            ..
        } = transaction;

        self.states.extend(created_states);
        self.derived.extend(created_derived);
        self.dynamic_model_owners.extend(created_model_owners);
        self.dynamic_model_types.extend(created_model_types);
        self.dynamic_model_owners.extend(updated_model_owners);

        let mut changed = Vec::new();

        for (name, value) in writes {
            let Some(state) = self.states.get_mut(&name) else {
                continue;
            };
            if !state_equivalent(&state.value, &value) {
                state.value = value;
                changed.push(name);
            }
        }

        for name in changed {
            self.invalidate_committed_dependency(&name);
        }

        for name in deleted_states.iter().chain(deleted_derived.iter()) {
            self.invalidate_committed_dependency(name);
        }
        for name in &deleted_derived {
            self.clear_committed_dependencies(name);
        }
        for name in deleted_derived {
            self.derived.remove(&name);
            self.reductions.remove(&name);
        }
        for name in deleted_states {
            self.states.remove(&name);
        }
        for identity in terminated_model_identities {
            self.dynamic_model_owners.remove(&identity);
            self.dynamic_model_types.remove(&identity);
        }
    }

    fn invalidate_committed_dependency(&mut self, dependency: &str) {
        let mut pending = vec![dependency.to_string()];
        let mut visited = HashSet::new();

        while let Some(dependency) = pending.pop() {
            if !visited.insert(dependency.clone()) {
                continue;
            }

            let watchers = if let Some(state) = self.states.get(&dependency) {
                state.dependents.clone()
            } else if let Some(derived) = self.derived.get(&dependency) {
                derived.dependents.clone()
            } else {
                HashSet::new()
            };

            for watcher in watchers {
                let was_valid = self
                    .derived
                    .get_mut(&watcher)
                    .and_then(|cell| cell.cached.take())
                    .is_some();
                if was_valid {
                    pending.push(watcher);
                }
            }
        }
    }
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Bool(_) => "Bool",
            Value::String(_) => "String",
            Value::Sequence { .. } => "ordered sequence",
        }
    }
}

fn model_binding_name(root: &str, member: &str) -> String {
    format!("__meld_sm${root}${member}")
}

fn coerce_value(value: Value, target: &ValueType) -> Result<Value, RuntimeError> {
    match (target, value) {
        (ValueType::Int, value @ Value::Int(_))
        | (ValueType::Bool, value @ Value::Bool(_))
        | (ValueType::String, value @ Value::String(_)) => Ok(value),
        (ValueType::Float, Value::Int(value)) => Ok(Value::Float(value as f64)),
        (ValueType::Float, value @ Value::Float(_)) => Ok(value),
        (
            ValueType::SequenceLive(expected_model),
            Value::Sequence {
                element_model,
                targets,
            },
        ) if expected_model == &element_model => Ok(Value::Sequence {
            element_model,
            targets,
        }),
        (ValueType::SequenceLive(expected_model), value) => Err(RuntimeError::new(format!(
            "cannot store {} in [live {}] value",
            value.type_name(),
            expected_model
        ))),
        (ValueType::Named(name), value) => Err(RuntimeError::new(format!(
            "runtime values for named type '{name}' are not implemented (got {})",
            value.type_name()
        ))),
        (target, value) => Err(RuntimeError::new(format!(
            "cannot store {} in {} value",
            value.type_name(),
            show_type(target)
        ))),
    }
}

fn is_zero_float(value: f64) -> bool {
    value.to_bits() & 0x7fff_ffff_ffff_ffff == 0
}

fn float_semantically_equal(left: f64, right: f64) -> bool {
    (left.is_nan() && right.is_nan())
        || (is_zero_float(left) && is_zero_float(right))
        || left.to_bits() == right.to_bits()
}

fn state_equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => float_semantically_equal(*a, *b),
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (
            Value::Sequence {
                element_model: a_model,
                targets: a_targets,
            },
            Value::Sequence {
                element_model: b_model,
                targets: b_targets,
            },
        ) => a_model == b_model && a_targets == b_targets,
        _ => false,
    }
}

fn apply_binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, RuntimeError> {
    match operator {
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide => apply_arithmetic(operator, left, right),
        BinaryOperator::And | BinaryOperator::Or => match (operator, left, right) {
            (BinaryOperator::And, Value::Bool(left), Value::Bool(right)) => {
                Ok(Value::Bool(left && right))
            }
            (BinaryOperator::Or, Value::Bool(left), Value::Bool(right)) => {
                Ok(Value::Bool(left || right))
            }
            (_, left, right) => Err(RuntimeError::new(format!(
                "operator {operator:?} is not defined for {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        },
        BinaryOperator::Contains => match (left, right) {
            (Value::String(haystack), Value::String(needle)) => {
                Ok(Value::Bool(haystack.contains(&needle)))
            }
            (left, right) => Err(RuntimeError::new(format!(
                "operator Contains is not defined for {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        },
        BinaryOperator::IsPresent => Err(RuntimeError::new(
            "internal maybe-live presence operator reached runtime before designation lowering",
        )),
        BinaryOperator::IsIn => match (left, right) {
            (Value::String(identity), Value::Sequence { targets, .. }) => {
                Ok(Value::Bool(targets.iter().any(|target| target == &identity)))
            }
            (left, right) => Err(RuntimeError::new(format!(
                "internal live-view membership operands must be designation and [live T], got {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        },
        BinaryOperator::PreviousIn | BinaryOperator::NextIn => match (left, right) {
            (Value::String(identity), Value::Sequence { targets, .. }) => {
                if identity.is_empty() {
                    return Err(RuntimeError::new(
                        "relative navigation anchor has no target",
                    ));
                }
                let direction = if operator == BinaryOperator::NextIn {
                    RelativeDirection::Next
                } else {
                    RelativeDirection::Previous
                };
                match resolve_unique_neighbor(&identity, &targets, direction) {
                    Ok(neighbor) => Ok(Value::String(neighbor)),
                    Err(RelativeNeighborError::NoCurrentOccurrence) => Err(RuntimeError::new(
                        "relative navigation anchor has no current occurrence in source",
                    )),
                    Err(RelativeNeighborError::AmbiguousCurrentOccurrence) => Err(
                        RuntimeError::new(
                            "relative navigation anchor has multiple current occurrences in source",
                        ),
                    ),
                    Err(RelativeNeighborError::Boundary) => Err(RuntimeError::new(
                        "relative navigation has no neighbor in the requested direction",
                    )),
                }
            }
            (left, right) => Err(RuntimeError::new(format!(
                "relative navigation requires live designation and [live T], got {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        },
        BinaryOperator::ContainsIgnoringCase => match (left, right) {
            (Value::String(haystack), Value::String(needle)) => {
                let folded_haystack = default_case_fold_str(&haystack);
                let folded_needle = default_case_fold_str(&needle);
                Ok(Value::Bool(folded_haystack.contains(&folded_needle)))
            }
            (left, right) => Err(RuntimeError::new(format!(
                "operator {operator:?} is not defined for {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        },
        BinaryOperator::Equal | BinaryOperator::NotEqual => apply_equality(operator, left, right),
        BinaryOperator::Less
        | BinaryOperator::LessEqual
        | BinaryOperator::Greater
        | BinaryOperator::GreaterEqual => apply_ordering(operator, left, right),
    }
}

fn apply_arithmetic(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, RuntimeError> {
    if operator == BinaryOperator::Add {
        if let (Value::String(left), Value::String(right)) = (&left, &right) {
            return Ok(Value::String(format!("{left}{right}")));
        }
    }

    match (left, right) {
        (Value::Int(left), Value::Int(right)) => apply_int_binary(operator, left, right),
        (Value::Int(left), Value::Float(right)) => apply_float_binary(operator, left as f64, right),
        (Value::Float(left), Value::Int(right)) => apply_float_binary(operator, left, right as f64),
        (Value::Float(left), Value::Float(right)) => apply_float_binary(operator, left, right),
        (left, right) => Err(RuntimeError::new(format!(
            "operator {operator:?} is not defined for {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

fn apply_equality(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, RuntimeError> {
    let equal = match (&left, &right) {
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            numeric_cmp(&left, &right) == Some(Ordering::Equal)
        }
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::String(left), Value::String(right)) => left == right,
        (
            Value::Sequence {
                element_model: left_model,
                targets: left_targets,
            },
            Value::Sequence {
                element_model: right_model,
                targets: right_targets,
            },
        ) => left_model == right_model && left_targets == right_targets,
        _ => {
            return Err(RuntimeError::new(format!(
                "operator {operator:?} is not defined for {} and {}",
                left.type_name(),
                right.type_name()
            )))
        }
    };

    Ok(Value::Bool(if operator == BinaryOperator::Equal {
        equal
    } else {
        !equal
    }))
}

fn apply_ordering(
    operator: BinaryOperator,
    left: Value,
    right: Value,
) -> Result<Value, RuntimeError> {
    if !matches!(left, Value::Int(_) | Value::Float(_))
        || !matches!(right, Value::Int(_) | Value::Float(_))
    {
        return Err(RuntimeError::new(format!(
            "operator {operator:?} is not defined for {} and {}",
            left.type_name(),
            right.type_name()
        )));
    }

    let Some(ordering) = numeric_cmp(&left, &right) else {
        return Ok(Value::Bool(false));
    };

    let result = match operator {
        BinaryOperator::Less => ordering == Ordering::Less,
        BinaryOperator::LessEqual => ordering != Ordering::Greater,
        BinaryOperator::Greater => ordering == Ordering::Greater,
        BinaryOperator::GreaterEqual => ordering != Ordering::Less,
        _ => unreachable!("non-ordering operator passed to ordering helper"),
    };

    Ok(Value::Bool(result))
}

fn numeric_cmp(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => Some(left.cmp(right)),
        (Value::Float(left), Value::Float(right)) => left.partial_cmp(right),
        (Value::Int(left), Value::Float(right)) => compare_i64_f64(*left, *right),
        (Value::Float(left), Value::Int(right)) => {
            compare_i64_f64(*right, *left).map(Ordering::reverse)
        }
        _ => None,
    }
}

fn compare_i64_f64(integer: i64, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }

    const I64_UPPER_EXCLUSIVE_AS_F64: f64 = 9_223_372_036_854_775_808.0;

    if float >= I64_UPPER_EXCLUSIVE_AS_F64 {
        return Some(Ordering::Less);
    }
    if float < -I64_UPPER_EXCLUSIVE_AS_F64 {
        return Some(Ordering::Greater);
    }

    let truncated = float.trunc() as i64;
    match integer.cmp(&truncated) {
        Ordering::Equal => {
            let truncated_float = truncated as f64;
            if float == truncated_float {
                Some(Ordering::Equal)
            } else if float > truncated_float {
                Some(Ordering::Less)
            } else {
                Some(Ordering::Greater)
            }
        }
        ordering => Some(ordering),
    }
}

fn apply_int_binary(
    operator: BinaryOperator,
    left: i64,
    right: i64,
) -> Result<Value, RuntimeError> {
    let value = match operator {
        BinaryOperator::Add => left.checked_add(right),
        BinaryOperator::Subtract => left.checked_sub(right),
        BinaryOperator::Multiply => left.checked_mul(right),
        BinaryOperator::Divide => {
            if right == 0 {
                return Err(RuntimeError::new("division by zero"));
            }
            left.checked_div(right)
        }
        _ => unreachable!("comparison operator passed to integer arithmetic helper"),
    }
    .ok_or_else(|| RuntimeError::new("integer arithmetic overflow"))?;

    Ok(Value::Int(value))
}

fn apply_float_binary(
    operator: BinaryOperator,
    left: f64,
    right: f64,
) -> Result<Value, RuntimeError> {
    if operator == BinaryOperator::Divide && is_zero_float(right) {
        return Err(RuntimeError::new("division by zero"));
    }

    let value = match operator {
        BinaryOperator::Add => left + right,
        BinaryOperator::Subtract => left - right,
        BinaryOperator::Multiply => left * right,
        BinaryOperator::Divide => left / right,
        _ => unreachable!("comparison operator passed to float arithmetic helper"),
    };
    Ok(Value::Float(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_with_dynamic_model() -> Runtime {
        let source = r#"
state model LineItem {
    state quantity = 2
    state unitPrice = 5.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice
state secondInvoice: Invoice

action reject {
    fail "reject created child"
}

action createLine {
    create LineItem in invoice
}

action createThenReject {
    create LineItem in invoice
    fail "reject source-created child"
}
"#;
        let checked = crate::check_source_with_runtime_models(source).expect("source should check");
        Runtime::from_checked_source(&checked).expect("runtime should initialize")
    }

    #[test]
    fn mixed_numeric_comparison_preserves_large_integer_precision() {
        let integer = Value::Int(9_007_199_254_740_993);
        let float = Value::Float(9_007_199_254_740_992.0);

        assert_eq!(numeric_cmp(&integer, &float), Some(Ordering::Greater));
        assert_eq!(
            apply_binary(BinaryOperator::Equal, integer.clone(), float.clone()).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            apply_binary(BinaryOperator::Greater, integer, float).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn nan_program_equality_differs_from_state_equivalence() {
        let nan = Value::Float(f64::NAN);

        assert!(state_equivalent(&nan, &nan));
        assert_eq!(
            apply_binary(BinaryOperator::Equal, nan.clone(), nan.clone()).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            apply_binary(BinaryOperator::NotEqual, nan.clone(), nan).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn signed_zero_is_equal_and_not_ordered() {
        let positive_zero = Value::Float(0.0);
        let negative_zero = Value::Float(-0.0);

        assert_eq!(
            apply_binary(
                BinaryOperator::Equal,
                positive_zero.clone(),
                negative_zero.clone(),
            )
            .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            apply_binary(BinaryOperator::Less, positive_zero, negative_zero).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn dynamic_model_creation_is_transaction_local_and_commits_as_one_identity() {
        let mut runtime = runtime_with_dynamic_model();
        runtime.transaction = Some(Transaction::default());

        let identity = runtime
            .instantiate_runtime_model("LineItem", "invoice")
            .expect("model should instantiate inside the transaction");
        let quantity = model_binding_name(&identity, "quantity");
        let total = model_binding_name(&identity, "lineTotal");

        assert_eq!(
            runtime
                .transaction
                .as_ref()
                .and_then(|transaction| transaction.created_model_owners.get(&identity))
                .map(String::as_str),
            Some("invoice")
        );
        assert!(!runtime.states.contains_key(&quantity));
        assert_eq!(runtime.read_name(&quantity, None).unwrap(), Value::Int(2));
        assert_eq!(runtime.read_name(&total, None).unwrap(), Value::Float(10.0));

        runtime
            .write_state(&quantity, Value::Int(3))
            .expect("fresh state should be writable transaction-locally");
        assert_eq!(runtime.read_name(&quantity, None).unwrap(), Value::Int(3));
        assert_eq!(runtime.read_name(&total, None).unwrap(), Value::Float(15.0));

        let transaction = runtime
            .transaction
            .take()
            .expect("transaction should exist");
        runtime.commit(transaction);

        assert_eq!(
            runtime
                .dynamic_model_owners
                .get(&identity)
                .map(String::as_str),
            Some("invoice")
        );
        assert_eq!(runtime.read_name(&quantity, None).unwrap(), Value::Int(3));
        assert_eq!(runtime.read_name(&total, None).unwrap(), Value::Float(15.0));
    }

    #[test]
    fn failed_transaction_discards_dynamic_model_identity_and_owner() {
        let mut runtime = runtime_with_dynamic_model();
        runtime.transaction = Some(Transaction::default());

        let identity = runtime
            .instantiate_runtime_model("LineItem", "invoice")
            .expect("model should instantiate inside the transaction");
        let quantity = model_binding_name(&identity, "quantity");
        assert_eq!(runtime.read_name(&quantity, None).unwrap(), Value::Int(2));

        let error = runtime
            .invoke_action("reject", &[])
            .expect_err("reject should fail the active transaction");
        assert!(error.message.contains("reject created child"));
        runtime.transaction = None;

        assert!(runtime.read_name(&quantity, None).is_err());
        assert!(!runtime.states.contains_key(&quantity));
        assert!(!runtime.dynamic_model_owners.contains_key(&identity));
    }

    #[test]
    fn owner_rooting_preserves_distinct_identity_without_membership_side_effects() {
        let mut runtime = runtime_with_dynamic_model();
        let sequence_count_before = runtime
            .states
            .values()
            .filter(|cell| matches!(cell.value, Value::Sequence { .. }))
            .count();
        runtime.transaction = Some(Transaction::default());
        let first = runtime
            .instantiate_runtime_model("LineItem", "invoice")
            .unwrap();
        let second = runtime
            .instantiate_runtime_model("LineItem", "invoice")
            .unwrap();
        let other_owner = runtime
            .instantiate_runtime_model("LineItem", "secondInvoice")
            .unwrap();
        assert_ne!(first, second);
        assert_ne!(first, other_owner);
        assert_ne!(second, other_owner);

        let transaction = runtime
            .transaction
            .take()
            .expect("transaction should exist");
        runtime.commit(transaction);

        let sequence_count_after = runtime
            .states
            .values()
            .filter(|cell| matches!(cell.value, Value::Sequence { .. }))
            .count();
        assert_eq!(sequence_count_after, sequence_count_before);
        assert_eq!(
            runtime.dynamic_model_owners.get(&first).map(String::as_str),
            Some("invoice")
        );
        assert_eq!(
            runtime
                .dynamic_model_owners
                .get(&second)
                .map(String::as_str),
            Some("invoice")
        );
        assert_eq!(
            runtime
                .dynamic_model_owners
                .get(&other_owner)
                .map(String::as_str),
            Some("secondInvoice")
        );

        assert_eq!(
            runtime
                .read_name(&model_binding_name(&first, "quantity"), None)
                .unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            runtime
                .read_name(&model_binding_name(&second, "quantity"), None)
                .unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            runtime
                .read_name(&model_binding_name(&other_owner, "quantity"), None)
                .unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn dynamic_model_creation_rejects_unknown_owner_root() {
        let mut runtime = runtime_with_dynamic_model();
        runtime.transaction = Some(Transaction::default());

        let error = runtime
            .instantiate_runtime_model("LineItem", "missing")
            .expect_err("unknown owner should be rejected");

        assert!(error
            .message
            .contains("unknown modeled-state owner 'missing'"));
    }

    #[test]
    fn source_create_commits_owner_rooted_identity() {
        let mut runtime = runtime_with_dynamic_model();
        assert!(runtime.dynamic_model_owners.is_empty());

        runtime
            .run_action("createLine")
            .expect("source create should run");

        assert_eq!(runtime.dynamic_model_owners.len(), 1);
        assert_eq!(
            runtime
                .dynamic_model_owners
                .values()
                .next()
                .map(String::as_str),
            Some("invoice")
        );
    }

    #[test]
    fn source_create_rolls_back_with_action_failure() {
        let mut runtime = runtime_with_dynamic_model();

        let error = runtime
            .run_action("createThenReject")
            .expect_err("failing action should reject source-created identity");

        assert!(error.message.contains("reject source-created child"));
        assert!(runtime.dynamic_model_owners.is_empty());
    }
}

#[cfg(test)]
mod relative_navigation_execution;
