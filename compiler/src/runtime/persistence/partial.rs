use std::collections::{HashMap, HashSet};

use super::*;

const MANIFEST_MAGIC: &[u8; 8] = b"ELANUPRT";
const BACKING_MAGIC: &[u8; 8] = b"ELANUBAK";
const MANIFEST_FORMAT_VERSION: u32 = 3;
const BACKING_FORMAT_VERSION: u32 = 1;

/// Host boundary for partially resident durable Elanu state.
///
/// The host stores only opaque bytes. Backing keys are runtime-owned byte
/// identifiers; providers must not infer model/member meaning from them.
/// `replace_candidate` must accept or reject the manifest and all supplied
/// backing replacements as one durable unit.
pub trait PartialPersistenceProvider {
    fn load_manifest(&mut self) -> Result<Option<Vec<u8>>, RuntimeError>;
    fn load_backing(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError>;
    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        backing_replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), RuntimeError>;
}

#[derive(Debug, Clone)]
struct BackingHandle {
    model_name: String,
    owner: String,
    token: u64,
    termination_cleanup_targets: HashSet<String>,
    resident_baseline: Option<Vec<u8>>,
}

/// Production runtime wrapper for durable worlds whose dynamic modeled state
/// may remain outside process memory until explicitly materialized.
///
/// Residency is a runtime/host concern only. Elanu source sees the same exact
/// modeled identities, designations, structure, lifetime provenance, and action
/// transaction semantics.
pub struct PartialPersistentRuntime<P> {
    runtime: Runtime,
    checked: CheckedSource,
    shape: PersistenceShape,
    provider: P,
    backed: HashMap<String, BackingHandle>,
    next_backing_token: u64,
}

impl<P: PartialPersistenceProvider> PartialPersistentRuntime<P> {
    pub fn open(checked: CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let shape = persistence_shape(&checked)?;
        let mut runtime = Runtime::from_checked_source(&checked)?;
        let mut backed = HashMap::new();
        let mut next_backing_token = 0;

        if let Some(bytes) = provider.load_manifest()? {
            let (partial, manifest_backed, manifest_next_token) = decode_manifest(&bytes)?;
            restore_image(&mut runtime, &shape, &partial)?;
            install_backed_metadata(&checked, &mut runtime, &manifest_backed)?;
            backed = manifest_backed;
            next_backing_token = manifest_next_token;
        }

        Ok(Self {
            runtime,
            checked,
            shape,
            provider,
            backed,
            next_backing_token,
        })
    }

    /// Return opaque keys for currently dormant backed identities.
    ///
    /// Keys carry no source/model/member semantics and are suitable only for
    /// passing back to `materialize` or to the provider's backing store.
    pub fn dormant_backing_keys(&self) -> Vec<Vec<u8>> {
        let mut keys = self
            .backed
            .iter()
            .filter(|(identity, _)| !identity_is_resident(&self.checked, &self.runtime, identity))
            .map(|(_, handle)| encode_key(handle.token))
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    fn designation_identity_and_handle(
        &self,
        designation: &str,
    ) -> Result<(String, BackingHandle), RuntimeError> {
        let lowered = self
            .checked
            .runtime_designation_bindings
            .get(designation)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "unknown top-level live designation '{designation}'"
                ))
            })?;
        let identity = match self
            .runtime
            .states
            .get(&lowered)
            .map(|cell| cell.value.clone())
        {
            Some(Value::String(identity)) if !identity.is_empty() => identity,
            Some(Value::String(_)) => {
                return Err(RuntimeError::new(format!(
                    "live designation '{designation}' has no target"
                )));
            }
            Some(other) => {
                let type_name = other.type_name();
                return Err(RuntimeError::new(format!(
                    "internal live designation '{designation}' carried {type_name}, expected String identity"
                )));
            }
            None => {
                return Err(RuntimeError::new(format!(
                    "runtime binding for live designation '{designation}' is missing"
                )));
            }
        };
        let handle = self.backed.get(&identity).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "live designation '{designation}' targets an identity without partial-persistence backing"
            ))
        })?;
        Ok((identity, handle))
    }

    /// Materialize the exact modeled identity currently carried by one
    /// top-level source designation. The host names application state, while
    /// dynamic identity and backing-key correlation remain runtime-private.
    pub fn materialize_designation(&mut self, designation: &str) -> Result<(), RuntimeError> {
        let (_, handle) = self.designation_identity_and_handle(designation)?;
        self.materialize(&encode_key(handle.token))
    }

    /// Return the number of current occurrences in one stored `[live T]`
    /// member of the model currently carried by a top-level designation.
    ///
    /// The observation explicitly materializes the designated owner as needed.
    /// Dynamic identities remain runtime-private.
    pub fn designation_member_len(
        &mut self,
        designation: &str,
        member: &str,
    ) -> Result<usize, RuntimeError> {
        let (_, targets) = self.designation_sequence_targets(designation, member)?;
        Ok(targets.len())
    }

    /// Observe one primitive stored member of the child at a current
    /// occurrence in a designation-owned `[live T]` sequence.
    ///
    /// The selected child is explicitly materialized as part of this
    /// observation. Structural sequences and live designations are rejected so
    /// runtime identity never crosses this host boundary.
    pub fn designation_member_index_value(
        &mut self,
        designation: &str,
        member: &str,
        index: usize,
        child_member: &str,
    ) -> Result<Value, RuntimeError> {
        let (expected_model, targets) = self.designation_sequence_targets(designation, member)?;
        let identity = targets.get(index).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "designation member index {index} is out of bounds for '{designation}.{member}' of length {}",
                targets.len()
            ))
        })?;

        let handle = self.backed.get(&identity).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "selected identity from '{designation}.{member}[{index}]' has no partial-persistence backing"
            ))
        })?;
        if handle.model_name != expected_model {
            return Err(RuntimeError::new(format!(
                "selected identity from '{designation}.{member}[{index}]' has model '{}' but sequence expects '{expected_model}'",
                handle.model_name
            )));
        }

        let child_metadata = stored_members(&self.checked, &handle)?
            .into_iter()
            .find(|candidate| candidate.name == child_member)
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "state model '{}' has no stored member '{child_member}'",
                    handle.model_name
                ))
            })?;
        if child_metadata.designation.is_some()
            || matches!(&child_metadata.value_type, ValueType::SequenceLive(_))
        {
            return Err(RuntimeError::new(format!(
                "host observation '{designation}.{member}[{index}].{child_member}' must name primitive stored state"
            )));
        }

        self.materialize(&encode_key(handle.token))?;
        let values = runtime_backing_values(&self.checked, &self.runtime, &identity, &handle)?;
        let value = values.get(child_member).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "resident backed identity '{identity}' is missing state '{child_member}'"
            ))
        })?;

        match value {
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::String(_) => Ok(value),
            Value::Sequence { .. } => Err(RuntimeError::new(format!(
                "host observation '{designation}.{member}[{index}].{child_member}' exposed structural identity"
            ))),
        }
    }

    fn designation_sequence_targets(
        &mut self,
        designation: &str,
        member: &str,
    ) -> Result<(String, Vec<String>), RuntimeError> {
        self.materialize_designation(designation)?;
        let (identity, handle) = self.designation_identity_and_handle(designation)?;

        let member_metadata = stored_members(&self.checked, &handle)?
            .into_iter()
            .find(|candidate| candidate.name == member)
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "state model '{}' has no stored member '{member}'",
                    handle.model_name
                ))
            })?;
        let expected_model = match &member_metadata.value_type {
            ValueType::SequenceLive(model) => model.clone(),
            other => {
                return Err(RuntimeError::new(format!(
                    "designation member '{designation}.{member}' must be [live T], got {}",
                    show_type(other)
                )));
            }
        };

        let values = runtime_backing_values(&self.checked, &self.runtime, &identity, &handle)?;
        let current = values.get(member).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "resident backed identity '{identity}' is missing state '{member}'"
            ))
        })?;
        let Value::Sequence {
            element_model,
            targets,
        } = current
        else {
            return Err(RuntimeError::new(format!(
                "designation member '{designation}.{member}' is not structural membership"
            )));
        };
        if element_model != expected_model {
            return Err(RuntimeError::new(format!(
                "designation member '{designation}.{member}' contains live {element_model} but expects live {expected_model}"
            )));
        }

        Ok((expected_model, targets))
    }

    /// Materialize the exact child identity at one current occurrence of a
    /// stored sequence owned by a statically declared modeled root. The host
    /// supplies only application-facing root/member/index selection facts;
    /// identity and backing-key correlation remain runtime-private.
    pub fn materialize_root_member_index(
        &mut self,
        root: &str,
        member: &str,
        index: usize,
    ) -> Result<(), RuntimeError> {
        let root_metadata = self
            .checked
            .runtime_model_roots
            .get(root)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown modeled root '{root}'")))?;
        let template = self
            .checked
            .runtime_model_templates
            .get(&root_metadata.model_name)
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "modeled root '{root}' references unknown model '{}'",
                    root_metadata.model_name
                ))
            })?;
        let member_metadata = template.member(member).ok_or_else(|| {
            RuntimeError::new(format!(
                "state model '{}' has no member '{member}'",
                template.name
            ))
        })?;
        if member_metadata.kind != crate::runtime_model_templates::RuntimeModelMemberKind::State {
            return Err(RuntimeError::new(format!(
                "modeled root selection '{root}.{member}' must name stored sequence state"
            )));
        }
        let expected_model = match &member_metadata.value_type {
            ValueType::SequenceLive(model) => model.clone(),
            other => {
                return Err(RuntimeError::new(format!(
                    "modeled root selection '{root}.{member}' must be [live T], got {}",
                    show_type(other)
                )));
            }
        };
        let state_name = self
            .checked
            .runtime_model_root_member_bindings
            .get(root)
            .and_then(|members| members.get(member))
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "runtime binding for modeled root selection '{root}.{member}' is missing"
                ))
            })?;
        let current = self
            .runtime
            .states
            .get(&state_name)
            .map(|cell| cell.value.clone())
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "runtime state for modeled root selection '{root}.{member}' is missing"
                ))
            })?;
        let Value::Sequence {
            element_model,
            targets,
        } = current
        else {
            return Err(RuntimeError::new(format!(
                "runtime state for modeled root selection '{root}.{member}' is not a sequence"
            )));
        };
        if element_model != expected_model {
            return Err(RuntimeError::new(format!(
                "modeled root selection '{root}.{member}' contains live {element_model} but expects live {expected_model}"
            )));
        }
        let identity = targets.get(index).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "modeled root selection index {index} is out of bounds for '{root}.{member}' of length {}",
                targets.len()
            ))
        })?;
        let handle = self.backed.get(&identity).ok_or_else(|| {
            RuntimeError::new(format!(
                "selected identity from '{root}.{member}[{index}]' has no partial-persistence backing"
            ))
        })?;
        if handle.model_name != expected_model {
            return Err(RuntimeError::new(format!(
                "selected identity from '{root}.{member}[{index}]' has model '{}' but sequence expects '{expected_model}'",
                handle.model_name
            )));
        }
        let key = encode_key(handle.token);
        self.materialize(&key)
    }

    /// Explicitly materialize one exact backed identity identified only by its
    /// opaque runtime key. Repeated materialization of an already resident
    /// identity is a no-op.
    pub fn materialize(&mut self, key: &[u8]) -> Result<(), RuntimeError> {
        let token = decode_key(key)?;
        let identity = self
            .backed
            .iter()
            .find_map(|(identity, handle)| (handle.token == token).then(|| identity.clone()))
            .ok_or_else(|| RuntimeError::new("unknown partial-persistence backing key"))?;

        if identity_is_resident(&self.checked, &self.runtime, &identity) {
            return Ok(());
        }

        let handle = self
            .backed
            .get(&identity)
            .cloned()
            .expect("backing identity was resolved from the same map");
        let payload = self
            .provider
            .load_backing(key)?
            .ok_or_else(|| RuntimeError::new("missing partial-persistence backing payload"))?;
        let values = decode_backing(&self.checked, &identity, &handle, &payload)?;
        let termination_cleanup_targets =
            termination_cleanup_targets_from_values(&self.checked, &handle, &values)?;
        if termination_cleanup_targets != handle.termination_cleanup_targets {
            return Err(RuntimeError::new(format!(
                "partial-persistence termination-cleanup summary for '{identity}' does not match backing"
            )));
        }
        install_member_values(
            &self.checked,
            &mut self.runtime,
            &identity,
            &handle,
            &values,
        )?;
        self.backed
            .get_mut(&identity)
            .expect("materialized identity should remain backed")
            .resident_baseline = Some(payload);
        Ok(())
    }

    pub fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        self.run_action_with_arguments(name, &[])
    }

    pub fn run_action_with_values(
        &mut self,
        name: &str,
        values: &[Value],
    ) -> Result<(), RuntimeError> {
        let arguments = values
            .iter()
            .map(host_value_action_argument)
            .collect::<Result<Vec<_>, _>>()?;

        self.run_action_with_arguments(name, &arguments)
    }

    fn run_action_with_arguments(
        &mut self,
        name: &str,
        arguments: &[ActionArgument],
    ) -> Result<(), RuntimeError> {
        if self.runtime.transaction.is_some() {
            return Err(RuntimeError::new(
                "partial persistent runtime cannot start a top-level action while a transaction is active",
            ));
        }

        let prior_next_dynamic_identity = self.runtime.next_dynamic_identity;
        let prior_partial =
            capture_partial_image(&self.checked, &self.runtime, &self.shape, &self.backed)?;
        let prior_backed = self.backed.clone();
        let prior_next_backing_token = self.next_backing_token;

        self.runtime.transaction = Some(Transaction::default());
        let result = self.runtime.invoke_action(name, arguments);
        let transaction = match result {
            Ok(()) => self
                .runtime
                .transaction
                .take()
                .expect("successful partial persistent action should retain its transaction"),
            Err(error) => {
                self.runtime.transaction = None;
                self.runtime.next_dynamic_identity = prior_next_dynamic_identity;
                return Err(error);
            }
        };

        let terminated_model_identities = transaction.terminated_model_identities.clone();
        let candidate_next_dynamic_identity = self.runtime.next_dynamic_identity;
        let mut candidate =
            restore_candidate_runtime(&self.checked, &self.shape, &prior_partial, &prior_backed)?;
        candidate.next_dynamic_identity = candidate_next_dynamic_identity;
        candidate.commit(transaction);

        let mut candidate_backed = prior_backed;
        let mut candidate_next_backing_token = prior_next_backing_token;
        synchronize_backing_handles(
            &candidate,
            &mut candidate_backed,
            &mut candidate_next_backing_token,
        )?;

        let mut replacements = Vec::new();
        if let Err(error) = self.rewrite_dormant_termination_cleanup(
            &candidate,
            &mut candidate_backed,
            &terminated_model_identities,
            &mut replacements,
        ) {
            self.runtime.next_dynamic_identity = prior_next_dynamic_identity;
            return Err(error);
        }

        let candidate_partial =
            capture_partial_image(&self.checked, &candidate, &self.shape, &candidate_backed)?;
        let mut identities = candidate_backed.keys().cloned().collect::<Vec<_>>();
        identities.sort();
        for identity in identities {
            if !identity_is_resident(&self.checked, &candidate, &identity) {
                continue;
            }
            let handle = candidate_backed
                .get(&identity)
                .cloned()
                .expect("candidate backing identity should still exist");
            let values = runtime_backing_values(&self.checked, &candidate, &identity, &handle)?;
            let payload = encode_backing_values(&self.checked, &identity, &handle, &values)?;
            let termination_cleanup_targets =
                termination_cleanup_targets_from_values(&self.checked, &handle, &values)?;
            let changed = handle
                .resident_baseline
                .as_ref()
                .map(|prior| prior != &payload)
                .unwrap_or(true);
            if changed {
                replacements.push((encode_key(handle.token), payload.clone()));
            }
            let candidate_handle = candidate_backed
                .get_mut(&identity)
                .expect("candidate backing identity should still exist");
            candidate_handle.termination_cleanup_targets = termination_cleanup_targets;
            candidate_handle.resident_baseline = Some(payload);
        }

        let manifest = encode_manifest(
            &candidate_partial,
            &candidate_backed,
            candidate_next_backing_token,
        )?;
        if let Err(error) = self.provider.replace_candidate(&manifest, &replacements) {
            self.runtime.next_dynamic_identity = prior_next_dynamic_identity;
            return Err(error);
        }

        self.runtime = candidate;
        self.backed = candidate_backed;
        self.next_backing_token = candidate_next_backing_token;
        Ok(())
    }

    fn rewrite_dormant_termination_cleanup(
        &mut self,
        candidate: &Runtime,
        candidate_backed: &mut HashMap<String, BackingHandle>,
        terminated_model_identities: &HashSet<String>,
        replacements: &mut Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), RuntimeError> {
        if terminated_model_identities.is_empty() {
            return Ok(());
        }

        let mut affected = candidate_backed
            .iter()
            .filter(|(identity, handle)| {
                !identity_is_resident(&self.checked, candidate, identity)
                    && !handle
                        .termination_cleanup_targets
                        .is_disjoint(terminated_model_identities)
            })
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        affected.sort();

        for identity in affected {
            let handle = candidate_backed
                .get(&identity)
                .cloned()
                .expect("affected dormant identity should remain backed");
            let key = encode_key(handle.token);
            let payload = self
                .provider
                .load_backing(&key)?
                .ok_or_else(|| RuntimeError::new("missing partial-persistence backing payload"))?;
            let mut values = decode_backing(&self.checked, &identity, &handle, &payload)?;
            let prior_targets =
                termination_cleanup_targets_from_values(&self.checked, &handle, &values)?;
            if prior_targets != handle.termination_cleanup_targets {
                return Err(RuntimeError::new(format!(
                    "partial-persistence termination-cleanup summary for '{identity}' does not match backing"
                )));
            }

            let mut changed = false;
            for member in stored_members(&self.checked, &handle)? {
                let value = values.get_mut(&member.name).ok_or_else(|| {
                    RuntimeError::new(format!(
                        "backing payload is missing '{identity}.{}'",
                        member.name
                    ))
                })?;
                if matches!(&member.value_type, ValueType::SequenceLive(_)) {
                    let Value::Sequence { targets, .. } = value else {
                        return Err(RuntimeError::new(format!(
                            "backing payload member '{identity}.{}' is not structural membership",
                            member.name
                        )));
                    };
                    let before = targets.len();
                    targets.retain(|target| !terminated_model_identities.contains(target));
                    changed |= targets.len() != before;
                    continue;
                }
                if member
                    .designation
                    .as_ref()
                    .is_some_and(|designation| designation.allows_none)
                {
                    let Value::String(target) = value else {
                        return Err(RuntimeError::new(format!(
                            "backing payload member '{identity}.{}' is not an optional live designation",
                            member.name
                        )));
                    };
                    if !target.is_empty() && terminated_model_identities.contains(target) {
                        target.clear();
                        changed = true;
                    }
                }
            }

            if !changed {
                return Err(RuntimeError::new(format!(
                    "partial-persistence termination-cleanup summary for '{identity}' reported a terminated target absent from backing"
                )));
            }

            let termination_cleanup_targets =
                termination_cleanup_targets_from_values(&self.checked, &handle, &values)?;
            if !termination_cleanup_targets.is_disjoint(terminated_model_identities) {
                return Err(RuntimeError::new(format!(
                    "partial-persistence termination cleanup for '{identity}' retained a terminated target"
                )));
            }
            let replacement = encode_backing_values(&self.checked, &identity, &handle, &values)?;
            replacements.push((key, replacement));
            candidate_backed
                .get_mut(&identity)
                .expect("affected dormant identity should remain backed")
                .termination_cleanup_targets = termination_cleanup_targets;
        }

        Ok(())
    }

    pub fn value(&mut self, name: &str) -> Result<Value, RuntimeError> {
        self.runtime.value(name)
    }

    pub fn derived_evaluations(&self, name: &str) -> Option<usize> {
        self.runtime.derived_evaluations(name)
    }

    pub fn into_provider(self) -> P {
        self.provider
    }
}

fn host_value_action_argument(value: &Value) -> Result<ActionArgument, RuntimeError> {
    let expression = match value {
        Value::Int(value) => Expr::Integer(*value),
        Value::Float(value) => Expr::Float(*value),
        Value::Bool(value) => Expr::Bool(*value),
        Value::String(value) => Expr::String(value.clone()),
        Value::Sequence { .. } => {
            return Err(RuntimeError::new(
                "partial persistent host action values cannot carry modeled identity sequences",
            ));
        }
    };

    Ok(ActionArgument::Value(expression))
}

fn encode_key(token: u64) -> Vec<u8> {
    token.to_le_bytes().to_vec()
}

fn decode_key(bytes: &[u8]) -> Result<u64, RuntimeError> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| RuntimeError::new("invalid partial-persistence backing key"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn encode_manifest(
    partial: &PersistenceImage,
    backed: &HashMap<String, BackingHandle>,
    next_backing_token: u64,
) -> Result<Vec<u8>, RuntimeError> {
    let partial_bytes = partial.encode()?;
    let mut encoder = PersistenceEncoder::default();
    encoder.raw(MANIFEST_MAGIC);
    encoder.u32(MANIFEST_FORMAT_VERSION);
    encoder.len(partial_bytes.len())?;
    encoder.raw(&partial_bytes);
    encoder.u64(next_backing_token);

    let mut identities = backed.keys().collect::<Vec<_>>();
    identities.sort();
    encoder.len(identities.len())?;
    for identity in identities {
        let handle = backed
            .get(identity)
            .expect("sorted backed identity should still exist");
        encoder.string(identity)?;
        encoder.string(&handle.model_name)?;
        encoder.string(&handle.owner)?;
        encoder.u64(handle.token);
        let mut cleanup_targets = handle
            .termination_cleanup_targets
            .iter()
            .collect::<Vec<_>>();
        cleanup_targets.sort();
        encoder.len(cleanup_targets.len())?;
        for target in cleanup_targets {
            encoder.string(target)?;
        }
    }
    Ok(encoder.finish())
}

fn decode_manifest(
    bytes: &[u8],
) -> Result<(PersistenceImage, HashMap<String, BackingHandle>, u64), RuntimeError> {
    let mut decoder = PersistenceDecoder::new(bytes);
    if decoder.raw(MANIFEST_MAGIC.len())? != MANIFEST_MAGIC {
        return Err(RuntimeError::new(
            "invalid partial-persistence manifest magic",
        ));
    }
    let version = decoder.u32()?;
    if version != MANIFEST_FORMAT_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported partial-persistence manifest version {version}"
        )));
    }

    let partial_len = decoder.len()?;
    let partial = PersistenceImage::decode(decoder.raw(partial_len)?)?;
    let next_backing_token = decoder.u64()?;
    let count = decoder.len()?;
    let mut backed = HashMap::new();
    let mut tokens = HashSet::new();
    for _ in 0..count {
        let identity = decoder.string()?;
        let model_name = decoder.string()?;
        let owner = decoder.string()?;
        let token = decoder.u64()?;
        let cleanup_count = decoder.len()?;
        let mut termination_cleanup_targets = HashSet::new();
        for _ in 0..cleanup_count {
            let target = decoder.string()?;
            if !termination_cleanup_targets.insert(target.clone()) {
                return Err(RuntimeError::new(format!(
                    "partial-persistence manifest repeats termination-cleanup target '{target}' for '{identity}'"
                )));
            }
        }
        let handle = BackingHandle {
            model_name,
            owner,
            token,
            termination_cleanup_targets,
            resident_baseline: None,
        };
        if !tokens.insert(handle.token) {
            return Err(RuntimeError::new(
                "partial-persistence manifest repeats a backing key",
            ));
        }
        if backed.insert(identity.clone(), handle).is_some() {
            return Err(RuntimeError::new(format!(
                "partial-persistence manifest repeats identity '{identity}'"
            )));
        }
    }
    decoder.finish()?;
    if tokens.iter().any(|token| *token >= next_backing_token) {
        return Err(RuntimeError::new(
            "partial-persistence manifest backing allocator is inconsistent",
        ));
    }
    Ok((partial, backed, next_backing_token))
}

fn stored_members<'a>(
    checked: &'a CheckedSource,
    handle: &BackingHandle,
) -> Result<Vec<&'a crate::runtime_model_templates::RuntimeModelMemberTemplate>, RuntimeError> {
    let template = checked
        .runtime_model_templates
        .get(&handle.model_name)
        .ok_or_else(|| RuntimeError::new(format!("unknown model '{}'", handle.model_name)))?;
    Ok(template
        .members
        .iter()
        .filter(|member| member.kind == RuntimeModelMemberKind::State)
        .collect())
}

fn identity_is_resident(checked: &CheckedSource, runtime: &Runtime, identity: &str) -> bool {
    let Some(model_name) = runtime.dynamic_model_types.get(identity) else {
        return false;
    };
    let Some(template) = checked.runtime_model_templates.get(model_name) else {
        return false;
    };
    template.members.iter().all(|member| {
        let name = model_binding_name(identity, &member.name);
        match member.kind {
            RuntimeModelMemberKind::State => runtime.states.contains_key(&name),
            RuntimeModelMemberKind::Derived => runtime.derived.contains_key(&name),
        }
    })
}

fn runtime_backing_values(
    checked: &CheckedSource,
    runtime: &Runtime,
    identity: &str,
    handle: &BackingHandle,
) -> Result<HashMap<String, Value>, RuntimeError> {
    let mut values = HashMap::new();
    for member in stored_members(checked, handle)? {
        let name = model_binding_name(identity, &member.name);
        let value = runtime
            .states
            .get(&name)
            .map(|cell| cell.value.clone())
            .ok_or_else(|| {
                RuntimeError::new(format!(
                    "resident backed identity '{identity}' is missing state '{}'",
                    member.name
                ))
            })?;
        values.insert(member.name.clone(), value);
    }
    Ok(values)
}

fn termination_cleanup_targets_from_values(
    checked: &CheckedSource,
    handle: &BackingHandle,
    values: &HashMap<String, Value>,
) -> Result<HashSet<String>, RuntimeError> {
    let mut cleanup_targets = HashSet::new();
    for member in stored_members(checked, handle)? {
        if matches!(&member.value_type, ValueType::SequenceLive(_)) {
            let value = values.get(&member.name).ok_or_else(|| {
                RuntimeError::new(format!(
                    "backing payload is missing stored structural member '{}'",
                    member.name
                ))
            })?;
            let Value::Sequence { targets, .. } = value else {
                return Err(RuntimeError::new(format!(
                    "backing payload stored member '{}' is not structural membership",
                    member.name
                )));
            };
            cleanup_targets.extend(targets.iter().cloned());
            continue;
        }
        if member
            .designation
            .as_ref()
            .is_some_and(|designation| designation.allows_none)
        {
            let value = values.get(&member.name).ok_or_else(|| {
                RuntimeError::new(format!(
                    "backing payload is missing stored optional designation member '{}'",
                    member.name
                ))
            })?;
            let Value::String(target) = value else {
                return Err(RuntimeError::new(format!(
                    "backing payload stored member '{}' is not an optional live designation",
                    member.name
                )));
            };
            if !target.is_empty() {
                cleanup_targets.insert(target.clone());
            }
        }
    }
    Ok(cleanup_targets)
}

fn encode_backing_values(
    checked: &CheckedSource,
    identity: &str,
    handle: &BackingHandle,
    values: &HashMap<String, Value>,
) -> Result<Vec<u8>, RuntimeError> {
    let members = stored_members(checked, handle)?;
    let mut encoder = PersistenceEncoder::default();
    encoder.raw(BACKING_MAGIC);
    encoder.u32(BACKING_FORMAT_VERSION);
    encoder.u64(handle.token);
    encoder.string(identity)?;
    encoder.string(&handle.model_name)?;
    encoder.len(members.len())?;
    for member in members {
        let value = values.get(&member.name).ok_or_else(|| {
            RuntimeError::new(format!(
                "backing payload is missing '{identity}.{}'",
                member.name
            ))
        })?;
        encode_value(&mut encoder, value)?;
    }
    Ok(encoder.finish())
}

fn decode_backing(
    checked: &CheckedSource,
    identity: &str,
    handle: &BackingHandle,
    bytes: &[u8],
) -> Result<HashMap<String, Value>, RuntimeError> {
    let mut decoder = PersistenceDecoder::new(bytes);
    if decoder.raw(BACKING_MAGIC.len())? != BACKING_MAGIC {
        return Err(RuntimeError::new(
            "invalid partial-persistence backing magic",
        ));
    }
    let version = decoder.u32()?;
    if version != BACKING_FORMAT_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported partial-persistence backing version {version}"
        )));
    }
    if decoder.u64()? != handle.token {
        return Err(RuntimeError::new(
            "partial-persistence backing key does not match request",
        ));
    }
    if decoder.string()? != identity {
        return Err(RuntimeError::new(
            "partial-persistence backing identity does not match request",
        ));
    }
    if decoder.string()? != handle.model_name {
        return Err(RuntimeError::new(
            "partial-persistence backing model does not match identity",
        ));
    }

    let members = stored_members(checked, handle)?;
    let count = decoder.len()?;
    if count != members.len() {
        return Err(RuntimeError::new(format!(
            "partial-persistence backing for '{identity}' has {count} states; model '{}' requires {}",
            handle.model_name,
            members.len()
        )));
    }
    let mut values = HashMap::new();
    for member in members {
        let value = coerce_value(decode_value(&mut decoder)?, &member.value_type)?;
        values.insert(member.name.clone(), value);
    }
    decoder.finish()?;
    Ok(values)
}

fn capture_partial_image(
    checked: &CheckedSource,
    runtime: &Runtime,
    shape: &PersistenceShape,
    backed: &HashMap<String, BackingHandle>,
) -> Result<PersistenceImage, RuntimeError> {
    let mut partial = capture_image(runtime, shape)?;
    for (identity, handle) in backed {
        if runtime.dynamic_model_types.get(identity) != Some(&handle.model_name) {
            return Err(RuntimeError::new(format!(
                "backed identity '{identity}' changed model type"
            )));
        }
        partial.dynamic_models.remove(identity);
        for member in stored_members(checked, handle)? {
            partial
                .state_values
                .remove(&model_binding_name(identity, &member.name));
        }
    }
    Ok(partial)
}

fn install_backed_metadata(
    checked: &CheckedSource,
    runtime: &mut Runtime,
    backed: &HashMap<String, BackingHandle>,
) -> Result<(), RuntimeError> {
    for (identity, handle) in backed {
        if runtime.dynamic_model_types.contains_key(identity)
            || runtime.dynamic_model_owners.contains_key(identity)
        {
            return Err(RuntimeError::new(format!(
                "backed identity '{identity}' duplicates a resident identity"
            )));
        }
        if !checked
            .runtime_model_templates
            .contains_key(&handle.model_name)
        {
            return Err(RuntimeError::new(format!(
                "backed identity '{identity}' references unknown model '{}'",
                handle.model_name
            )));
        }
        let owner_exists = runtime.runtime_model_roots.contains_key(&handle.owner)
            || runtime.dynamic_model_types.contains_key(&handle.owner)
            || backed.contains_key(&handle.owner);
        if !owner_exists {
            return Err(RuntimeError::new(format!(
                "backed identity '{identity}' references unknown owner '{}'",
                handle.owner
            )));
        }
        runtime
            .dynamic_model_types
            .insert(identity.clone(), handle.model_name.clone());
        runtime
            .dynamic_model_owners
            .insert(identity.clone(), handle.owner.clone());
    }
    Ok(())
}

fn install_member_values(
    checked: &CheckedSource,
    runtime: &mut Runtime,
    identity: &str,
    handle: &BackingHandle,
    values: &HashMap<String, Value>,
) -> Result<(), RuntimeError> {
    if runtime.dynamic_model_types.get(identity) != Some(&handle.model_name) {
        return Err(RuntimeError::new("backed identity changed model type"));
    }
    if runtime.dynamic_model_owners.get(identity) != Some(&handle.owner) {
        return Err(RuntimeError::new("backed identity changed lifetime owner"));
    }
    let template = checked
        .runtime_model_templates
        .get(&handle.model_name)
        .cloned()
        .expect("backed model was validated before materialization");
    let context = ModelRuntimeContext {
        root: identity.to_string(),
        members: template
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect(),
    };
    let mut states = Vec::new();
    let mut derived = Vec::new();
    for member in &template.members {
        let name = model_binding_name(identity, &member.name);
        if runtime.states.contains_key(&name) || runtime.derived.contains_key(&name) {
            return Err(RuntimeError::new(format!(
                "materialization would duplicate member '{identity}.{}'",
                member.name
            )));
        }
        match member.kind {
            RuntimeModelMemberKind::State => states.push((
                name,
                StateCell {
                    value: values
                        .get(&member.name)
                        .cloned()
                        .expect("validated backing should contain each stored member"),
                    value_type: member.value_type.clone(),
                    designation: member.designation.clone(),
                    dependents: HashSet::new(),
                },
            )),
            RuntimeModelMemberKind::Derived => derived.push((
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
            )),
        }
    }
    for (name, state) in states {
        runtime.states.insert(name, state);
    }
    for (name, cell) in derived {
        runtime.derived.insert(name, cell);
    }
    Ok(())
}

fn restore_candidate_runtime(
    checked: &CheckedSource,
    shape: &PersistenceShape,
    partial: &PersistenceImage,
    backed: &HashMap<String, BackingHandle>,
) -> Result<Runtime, RuntimeError> {
    let mut runtime = Runtime::from_checked_source(checked)?;
    restore_image(&mut runtime, shape, partial)?;
    install_backed_metadata(checked, &mut runtime, backed)?;
    let mut identities = backed.keys().cloned().collect::<Vec<_>>();
    identities.sort();
    for identity in identities {
        let handle = backed
            .get(&identity)
            .expect("candidate backed identity should exist");
        if let Some(payload) = &handle.resident_baseline {
            let values = decode_backing(checked, &identity, handle, payload)?;
            let termination_cleanup_targets =
                termination_cleanup_targets_from_values(checked, handle, &values)?;
            if termination_cleanup_targets != handle.termination_cleanup_targets {
                return Err(RuntimeError::new(format!(
                    "partial-persistence termination-cleanup summary for '{identity}' does not match resident baseline"
                )));
            }
            install_member_values(checked, &mut runtime, &identity, handle, &values)?;
        }
    }
    Ok(runtime)
}

fn synchronize_backing_handles(
    runtime: &Runtime,
    backed: &mut HashMap<String, BackingHandle>,
    next_backing_token: &mut u64,
) -> Result<(), RuntimeError> {
    backed.retain(|identity, _| runtime.dynamic_model_types.contains_key(identity));

    let mut identities = runtime
        .dynamic_model_types
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    identities.sort();
    for identity in identities {
        let model_name = runtime
            .dynamic_model_types
            .get(&identity)
            .cloned()
            .expect("dynamic identity should retain its model type");
        let owner = runtime
            .dynamic_model_owners
            .get(&identity)
            .cloned()
            .ok_or_else(|| RuntimeError::new("dynamic identity is missing lifetime owner"))?;
        if let Some(handle) = backed.get_mut(&identity) {
            if handle.model_name != model_name {
                return Err(RuntimeError::new("backed identity changed model type"));
            }
            handle.owner = owner;
        } else {
            let token = *next_backing_token;
            *next_backing_token = next_backing_token.checked_add(1).ok_or_else(|| {
                RuntimeError::new("partial-persistence backing key space exhausted")
            })?;
            backed.insert(
                identity,
                BackingHandle {
                    model_name,
                    owner,
                    token,
                    termination_cleanup_targets: HashSet::new(),
                    resident_baseline: None,
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state name = ""
    state folders: [live Folder] = []
    state documents: [live Document] = []
}

state workspace: Folder
state activeFolder: maybe live Folder = none
state coldFolder: maybe live Folder = none
state audit = "idle"

derived coldName = coldFolder.name
derived coldDocumentTitle = coldFolder.documents[0].title

action seed {
    create Folder in workspace as active {
        through active.name = "Active"
        insert active into workspace.folders
    }
    activeFolder = workspace.folders[0]
    create Folder in workspace as cold {
        through cold.name = "Cold"
        insert cold into workspace.folders
    }
    coldFolder = workspace.folders[1]
    create Document in coldFolder as doc {
        through doc.title = "Cold document"
        insert doc into coldFolder.documents
    }
}

action auditOnly {
    audit = "audited"
}

action duplicateColdOccurrence {
    insert coldFolder into workspace.folders
}

action renameCold {
    through coldFolder.name = "Cold renamed"
    audit = "renamed cold"
}

action failedRenameCold {
    through coldFolder.name = "Wrong"
    audit = "wrong"
    fail "abort"
}
"#;

    #[derive(Debug, Clone, Default)]
    struct MemoryProvider {
        manifest: Option<Vec<u8>>,
        backing: HashMap<Vec<u8>, Vec<u8>>,
        loads: Vec<Vec<u8>>,
        replacements: Vec<Vec<Vec<u8>>>,
        reject_next: bool,
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
            if self.reject_next {
                self.reject_next = false;
                return Err(RuntimeError::new("provider rejected candidate"));
            }
            let mut candidate = self.backing.clone();
            for (key, payload) in backing_replacements {
                candidate.insert(key.clone(), payload.clone());
            }
            self.backing = candidate;
            self.manifest = Some(manifest.to_vec());
            self.replacements.push(
                backing_replacements
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect(),
            );
            Ok(())
        }
    }

    fn checked() -> CheckedSource {
        crate::check_source_with_runtime_models(SOURCE)
            .expect("partial persistence source should check")
    }

    fn seeded_provider() -> (CheckedSource, MemoryProvider) {
        let checked = checked();
        let mut runtime =
            PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
                .expect("fresh partial runtime should open");
        runtime.run_action("seed").expect("seed should publish");
        (checked, runtime.into_provider())
    }

    fn keys_by_model(runtime: &PartialPersistentRuntime<MemoryProvider>) -> (Vec<u8>, Vec<u8>) {
        let folder = runtime
            .backed
            .iter()
            .find_map(|(identity, handle)| {
                if handle.model_name != "Folder" {
                    return None;
                }
                let key = encode_key(handle.token);
                let payload = runtime.provider.backing.get(&key)?;
                let values = decode_backing(&runtime.checked, identity, handle, payload).ok()?;
                matches!(values.get("name"), Some(Value::String(name)) if name == "Cold")
                    .then_some(key)
            })
            .expect("Cold Folder backing should exist");
        let document = runtime
            .backed
            .iter()
            .find_map(|(_, handle)| {
                (handle.model_name == "Document").then(|| encode_key(handle.token))
            })
            .expect("Document backing should exist");
        (folder, document)
    }

    #[test]
    fn restart_is_dormant_and_resident_only_publication_reuses_backing() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        provider.replacements.clear();
        let backing_before = provider.backing.clone();

        let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider).unwrap();
        assert!(runtime.provider.loads.is_empty());
        assert!(!runtime.dormant_backing_keys().is_empty());
        runtime.run_action("auditOnly").unwrap();
        assert_eq!(
            runtime.value("audit").unwrap(),
            Value::String("audited".into())
        );
        assert!(runtime.provider.loads.is_empty());
        assert_eq!(runtime.provider.backing, backing_before);
        assert_eq!(runtime.provider.replacements, vec![Vec::<Vec<u8>>::new()]);

        let mut provider = runtime.into_provider();
        provider.loads.clear();
        let mut restarted = PartialPersistentRuntime::open(checked, provider).unwrap();
        assert_eq!(
            restarted.value("audit").unwrap(),
            Value::String("audited".into())
        );
        assert!(restarted.provider.loads.is_empty());
    }

    #[test]
    fn designation_relative_observation_materializes_only_requested_children() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (folder_key, document_key) = keys_by_model(&runtime);

        assert_eq!(
            runtime
                .designation_member_len("coldFolder", "documents")
                .unwrap(),
            1
        );
        assert_eq!(runtime.provider.loads, vec![folder_key.clone()]);

        assert_eq!(
            runtime
                .designation_member_index_value("coldFolder", "documents", 0, "title",)
                .unwrap(),
            Value::String("Cold document".into())
        );
        assert_eq!(runtime.provider.loads, vec![folder_key, document_key]);
    }

    #[test]
    fn structural_index_materializes_exact_selected_dormant_identity() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (cold_key, document_key) = keys_by_model(&runtime);

        runtime
            .materialize_root_member_index("workspace", "folders", 1)
            .unwrap();
        assert_eq!(runtime.provider.loads, vec![cold_key]);
        assert!(!runtime.provider.loads.contains(&document_key));
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold".into())
        );
    }

    #[test]
    fn invalid_structural_index_fails_without_backing_read() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();

        runtime
            .materialize_root_member_index("workspace", "folders", 99)
            .expect_err("out-of-range structural selection should fail");
        assert!(runtime.provider.loads.is_empty());
    }

    #[test]
    fn duplicate_occurrences_materialize_one_child_identity() {
        let (checked, provider) = seeded_provider();
        let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider).unwrap();
        runtime
            .materialize_designation("coldFolder")
            .expect("Cold Folder should materialize before duplicate insertion");
        runtime
            .run_action("duplicateColdOccurrence")
            .expect("duplicate structural occurrence should publish");
        let mut provider = runtime.into_provider();
        provider.loads.clear();
        let mut restarted = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (cold_key, _) = keys_by_model(&restarted);

        restarted
            .materialize_root_member_index("workspace", "folders", 2)
            .unwrap();
        assert_eq!(restarted.provider.loads, vec![cold_key.clone()]);
        restarted
            .materialize_root_member_index("workspace", "folders", 1)
            .unwrap();
        assert_eq!(restarted.provider.loads, vec![cold_key]);
    }

    #[test]
    fn source_designation_materializes_exact_selected_dormant_identity() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (folder_key, document_key) = keys_by_model(&runtime);

        runtime.materialize_designation("coldFolder").unwrap();
        assert_eq!(runtime.provider.loads, vec![folder_key]);
        assert!(!runtime.provider.loads.contains(&document_key));
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold".into())
        );
    }

    #[test]
    fn designation_materialization_failure_does_not_partially_install_members() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let (folder_key, _) = {
            let runtime =
                PartialPersistentRuntime::open(checked.clone(), provider.clone()).unwrap();
            keys_by_model(&runtime)
        };
        provider.backing.remove(&folder_key);
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();

        runtime
            .materialize_designation("coldFolder")
            .expect_err("missing backing should reject selected materialization");
        assert!(runtime.value("coldName").is_err());
        assert_eq!(runtime.provider.loads, vec![folder_key]);
    }

    #[test]
    fn one_materialized_identity_changes_without_touching_unrelated_backing() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        provider.replacements.clear();
        let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider).unwrap();
        let (folder_key, document_key) = keys_by_model(&runtime);
        let document_before = runtime.provider.backing[&document_key].clone();

        runtime.materialize(&folder_key).unwrap();
        assert_eq!(runtime.provider.loads, vec![folder_key.clone()]);
        runtime.run_action("renameCold").unwrap();
        assert_eq!(
            runtime.value("audit").unwrap(),
            Value::String("renamed cold".into())
        );
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold renamed".into())
        );
        assert_eq!(runtime.provider.loads, vec![folder_key.clone()]);
        assert_eq!(
            runtime.provider.replacements.last().unwrap(),
            &vec![folder_key.clone()]
        );
        assert_eq!(runtime.provider.backing[&document_key], document_before);

        let mut provider = runtime.into_provider();
        provider.loads.clear();
        let mut restarted = PartialPersistentRuntime::open(checked, provider).unwrap();
        assert!(restarted.provider.loads.is_empty());
        restarted.materialize(&folder_key).unwrap();
        assert_eq!(restarted.provider.loads, vec![folder_key]);
        assert_eq!(
            restarted.value("coldName").unwrap(),
            Value::String("Cold renamed".into())
        );
        assert_eq!(restarted.provider.backing[&document_key], document_before);
    }

    #[test]
    fn rejection_and_semantic_failure_preserve_prior_world() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        provider.replacements.clear();
        let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider).unwrap();
        let (folder_key, _) = keys_by_model(&runtime);
        runtime.materialize(&folder_key).unwrap();
        let manifest_before = runtime.provider.manifest.clone();
        let backing_before = runtime.provider.backing.clone();
        runtime.provider.reject_next = true;

        runtime
            .run_action("renameCold")
            .expect_err("provider rejection should reject action");
        assert_eq!(runtime.provider.manifest, manifest_before);
        assert_eq!(runtime.provider.backing, backing_before);
        assert_eq!(
            runtime.value("audit").unwrap(),
            Value::String("idle".into())
        );
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold".into())
        );

        runtime
            .run_action("failedRenameCold")
            .expect_err("semantic failure should abort");
        assert_eq!(runtime.provider.manifest, manifest_before);
        assert_eq!(runtime.provider.backing, backing_before);
        assert_eq!(
            runtime.value("audit").unwrap(),
            Value::String("idle".into())
        );
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold".into())
        );
    }
}
