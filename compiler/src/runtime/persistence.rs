use std::collections::{HashMap, HashSet};

use super::*;

const PERSISTENCE_ENCODING_MAGIC: &[u8; 8] = b"ELANUPST";
const PERSISTENCE_ENCODING_VERSION: u32 = 1;

impl PersistenceImage {
    /// Encode this image into a runtime-owned opaque byte representation.
    ///
    /// The byte format is an implementation compatibility boundary, not Elanu
    /// source syntax or a provider-facing schema.
    pub fn encode(&self) -> Result<Vec<u8>, RuntimeError> {
        let mut encoder = PersistenceEncoder::default();
        encoder.raw(PERSISTENCE_ENCODING_MAGIC);
        encoder.u32(PERSISTENCE_ENCODING_VERSION);
        encode_shape(&mut encoder, &self.shape)?;
        encode_state_values(&mut encoder, &self.state_values)?;
        encode_dynamic_models(&mut encoder, &self.dynamic_models)?;
        encoder.u64(self.next_dynamic_identity);
        Ok(encoder.finish())
    }

    /// Decode one runtime-owned opaque persistence byte payload.
    ///
    /// Encoding validation happens here. Checked-application compatibility is a
    /// separate later check performed by `PersistentRuntime::open`.
    pub fn decode(bytes: &[u8]) -> Result<Self, RuntimeError> {
        let mut decoder = PersistenceDecoder::new(bytes);
        let magic = decoder.raw(PERSISTENCE_ENCODING_MAGIC.len())?;
        if magic != PERSISTENCE_ENCODING_MAGIC {
            return Err(RuntimeError::new("invalid persistence encoding magic"));
        }

        let version = decoder.u32()?;
        if version != PERSISTENCE_ENCODING_VERSION {
            return Err(RuntimeError::new(format!(
                "unsupported persistence encoding version {version}; runtime supports version {PERSISTENCE_ENCODING_VERSION}"
            )));
        }

        let shape = decode_shape(&mut decoder)?;
        let state_values = decode_state_values(&mut decoder)?;
        let dynamic_models = decode_dynamic_models(&mut decoder)?;
        let next_dynamic_identity = decoder.u64()?;
        decoder.finish()?;

        Ok(Self {
            shape,
            state_values,
            dynamic_models,
            next_dynamic_identity,
        })
    }
}

#[derive(Default)]
struct PersistenceEncoder {
    bytes: Vec<u8>,
}

impl PersistenceEncoder {
    fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.raw(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.raw(&value.to_le_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.raw(&value.to_le_bytes());
    }

    fn len(&mut self, value: usize) -> Result<(), RuntimeError> {
        let value = u32::try_from(value)
            .map_err(|_| RuntimeError::new("persistence encoding collection is too large"))?;
        self.u32(value);
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), RuntimeError> {
        self.len(value.len())?;
        self.raw(value.as_bytes());
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

struct PersistenceDecoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PersistenceDecoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn raw(&mut self, len: usize) -> Result<&'a [u8], RuntimeError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| RuntimeError::new("invalid persistence encoding length overflow"))?;
        if end > self.bytes.len() {
            return Err(RuntimeError::new("truncated persistence encoding"));
        }
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, RuntimeError> {
        Ok(self.raw(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, RuntimeError> {
        let bytes: [u8; 4] = self
            .raw(4)?
            .try_into()
            .expect("decoder requested exactly four bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, RuntimeError> {
        let bytes: [u8; 8] = self
            .raw(8)?
            .try_into()
            .expect("decoder requested exactly eight bytes");
        Ok(u64::from_le_bytes(bytes))
    }

    fn i64(&mut self) -> Result<i64, RuntimeError> {
        let bytes: [u8; 8] = self
            .raw(8)?
            .try_into()
            .expect("decoder requested exactly eight bytes");
        Ok(i64::from_le_bytes(bytes))
    }

    fn len(&mut self) -> Result<usize, RuntimeError> {
        usize::try_from(self.u32()?)
            .map_err(|_| RuntimeError::new("persistence encoding length is unsupported"))
    }

    fn string(&mut self) -> Result<String, RuntimeError> {
        let len = self.len()?;
        let bytes = self.raw(len)?;
        let value = std::str::from_utf8(bytes)
            .map_err(|_| RuntimeError::new("persistence encoding contains invalid UTF-8"))?;
        Ok(value.to_owned())
    }

    fn finish(&self) -> Result<(), RuntimeError> {
        if self.offset != self.bytes.len() {
            return Err(RuntimeError::new(
                "persistence encoding contains trailing bytes",
            ));
        }
        Ok(())
    }
}

fn encode_shape(
    encoder: &mut PersistenceEncoder,
    shape: &PersistenceShape,
) -> Result<(), RuntimeError> {
    encoder.len(shape.static_states.len())?;
    for state in &shape.static_states {
        encoder.string(&state.name)?;
        encode_value_type(encoder, &state.value_type)?;
        encode_designation(encoder, state.designation.as_ref())?;
    }

    encoder.len(shape.model_states.len())?;
    for state in &shape.model_states {
        encoder.string(&state.model_name)?;
        encoder.string(&state.member_name)?;
        encode_value_type(encoder, &state.value_type)?;
        encode_designation(encoder, state.designation.as_ref())?;
    }

    encoder.len(shape.roots.len())?;
    for root in &shape.roots {
        encoder.string(&root.root_name)?;
        encoder.string(&root.model_name)?;
    }
    Ok(())
}

fn decode_shape(decoder: &mut PersistenceDecoder<'_>) -> Result<PersistenceShape, RuntimeError> {
    let static_count = decoder.len()?;
    let mut static_states = Vec::with_capacity(static_count);
    for _ in 0..static_count {
        static_states.push(PersistenceStateShape {
            name: decoder.string()?,
            value_type: decode_value_type(decoder)?,
            designation: decode_designation(decoder)?,
        });
    }

    let model_count = decoder.len()?;
    let mut model_states = Vec::with_capacity(model_count);
    for _ in 0..model_count {
        model_states.push(PersistenceModelStateShape {
            model_name: decoder.string()?,
            member_name: decoder.string()?,
            value_type: decode_value_type(decoder)?,
            designation: decode_designation(decoder)?,
        });
    }

    let root_count = decoder.len()?;
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        roots.push(PersistenceRootShape {
            root_name: decoder.string()?,
            model_name: decoder.string()?,
        });
    }

    Ok(PersistenceShape {
        static_states,
        model_states,
        roots,
    })
}

fn encode_designation(
    encoder: &mut PersistenceEncoder,
    designation: Option<&RuntimeDesignationMetadata>,
) -> Result<(), RuntimeError> {
    match designation {
        None => encoder.u8(0),
        Some(designation) => {
            encoder.u8(1);
            encoder.string(&designation.model_name)?;
            encoder.u8(u8::from(designation.allows_none));
        }
    }
    Ok(())
}

fn decode_designation(
    decoder: &mut PersistenceDecoder<'_>,
) -> Result<Option<RuntimeDesignationMetadata>, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(RuntimeDesignationMetadata {
            model_name: decoder.string()?,
            allows_none: decode_bool(decoder)?,
        })),
        tag => Err(RuntimeError::new(format!(
            "invalid persistence designation tag {tag}"
        ))),
    }
}

fn encode_value_type(
    encoder: &mut PersistenceEncoder,
    value_type: &ValueType,
) -> Result<(), RuntimeError> {
    match value_type {
        ValueType::Int => encoder.u8(0),
        ValueType::Float => encoder.u8(1),
        ValueType::Bool => encoder.u8(2),
        ValueType::String => encoder.u8(3),
        ValueType::SequenceLive(model) => {
            encoder.u8(4);
            encoder.string(model)?;
        }
        ValueType::Named(name) => {
            encoder.u8(5);
            encoder.string(name)?;
        }
    }
    Ok(())
}

fn decode_value_type(decoder: &mut PersistenceDecoder<'_>) -> Result<ValueType, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(ValueType::Int),
        1 => Ok(ValueType::Float),
        2 => Ok(ValueType::Bool),
        3 => Ok(ValueType::String),
        4 => Ok(ValueType::SequenceLive(decoder.string()?)),
        5 => Ok(ValueType::Named(decoder.string()?)),
        tag => Err(RuntimeError::new(format!(
            "invalid persistence value-type tag {tag}"
        ))),
    }
}

fn encode_value(encoder: &mut PersistenceEncoder, value: &Value) -> Result<(), RuntimeError> {
    match value {
        Value::Int(value) => {
            encoder.u8(0);
            encoder.i64(*value);
        }
        Value::Float(value) => {
            encoder.u8(1);
            encoder.u64(value.to_bits());
        }
        Value::Bool(value) => {
            encoder.u8(2);
            encoder.u8(u8::from(*value));
        }
        Value::String(value) => {
            encoder.u8(3);
            encoder.string(value)?;
        }
        Value::Sequence {
            element_model,
            targets,
        } => {
            encoder.u8(4);
            encoder.string(element_model)?;
            encoder.len(targets.len())?;
            for target in targets {
                encoder.string(target)?;
            }
        }
    }
    Ok(())
}

fn decode_value(decoder: &mut PersistenceDecoder<'_>) -> Result<Value, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(Value::Int(decoder.i64()?)),
        1 => Ok(Value::Float(f64::from_bits(decoder.u64()?))),
        2 => Ok(Value::Bool(decode_bool(decoder)?)),
        3 => Ok(Value::String(decoder.string()?)),
        4 => {
            let element_model = decoder.string()?;
            let count = decoder.len()?;
            let mut targets = Vec::with_capacity(count);
            for _ in 0..count {
                targets.push(decoder.string()?);
            }
            Ok(Value::Sequence {
                element_model,
                targets,
            })
        }
        tag => Err(RuntimeError::new(format!(
            "invalid persistence value tag {tag}"
        ))),
    }
}

fn decode_bool(decoder: &mut PersistenceDecoder<'_>) -> Result<bool, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(RuntimeError::new(format!(
            "invalid persistence boolean value {value}"
        ))),
    }
}

fn encode_state_values(
    encoder: &mut PersistenceEncoder,
    values: &HashMap<String, Value>,
) -> Result<(), RuntimeError> {
    let mut names = values.keys().collect::<Vec<_>>();
    names.sort();
    encoder.len(names.len())?;
    for name in names {
        encoder.string(name)?;
        encode_value(
            encoder,
            values
                .get(name)
                .expect("sorted persistence state name should still exist"),
        )?;
    }
    Ok(())
}

fn decode_state_values(
    decoder: &mut PersistenceDecoder<'_>,
) -> Result<HashMap<String, Value>, RuntimeError> {
    let count = decoder.len()?;
    let mut values = HashMap::with_capacity(count);
    for _ in 0..count {
        let name = decoder.string()?;
        let value = decode_value(decoder)?;
        if values.insert(name.clone(), value).is_some() {
            return Err(RuntimeError::new(format!(
                "persistence encoding repeats state '{name}'"
            )));
        }
    }
    Ok(values)
}

fn encode_dynamic_models(
    encoder: &mut PersistenceEncoder,
    models: &HashMap<String, PersistenceDynamicModel>,
) -> Result<(), RuntimeError> {
    let mut identities = models.keys().collect::<Vec<_>>();
    identities.sort();
    encoder.len(identities.len())?;
    for identity in identities {
        let model = models
            .get(identity)
            .expect("sorted dynamic persistence identity should still exist");
        encoder.string(identity)?;
        encoder.string(&model.model_name)?;
        encoder.string(&model.owner)?;
    }
    Ok(())
}

fn decode_dynamic_models(
    decoder: &mut PersistenceDecoder<'_>,
) -> Result<HashMap<String, PersistenceDynamicModel>, RuntimeError> {
    let count = decoder.len()?;
    let mut models = HashMap::with_capacity(count);
    for _ in 0..count {
        let identity = decoder.string()?;
        let model = PersistenceDynamicModel {
            model_name: decoder.string()?,
            owner: decoder.string()?,
        };
        if models.insert(identity.clone(), model).is_some() {
            return Err(RuntimeError::new(format!(
                "persistence encoding repeats dynamic identity '{identity}'"
            )));
        }
    }
    Ok(models)
}

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
    struct ByteMemoryProvider {
        bytes: Option<Vec<u8>>,
        replace_attempts: usize,
    }

    impl PersistenceProvider for ByteMemoryProvider {
        fn load(&mut self) -> Result<Option<PersistenceImage>, RuntimeError> {
            self.bytes
                .as_deref()
                .map(PersistenceImage::decode)
                .transpose()
        }

        fn replace(&mut self, image: &PersistenceImage) -> Result<(), RuntimeError> {
            self.replace_attempts += 1;
            self.bytes = Some(image.encode()?);
            Ok(())
        }
    }

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
    fn opaque_bytes_round_trip_across_provider_recreation() {
        let mut first = PersistentRuntime::open(checked(), ByteMemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        let provider = first.into_provider();
        let durable_bytes = provider
            .bytes
            .expect("accepted action should produce bytes");

        // Process-equivalent restart: only the byte payload survives. Neither the
        // prior runtime nor a PersistenceImage object is carried across.
        let provider = ByteMemoryProvider {
            bytes: Some(durable_bytes),
            replace_attempts: 0,
        };
        let mut restarted = PersistentRuntime::open(checked(), provider)
            .expect("runtime-owned bytes should decode and restore");

        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
        restarted.run_action("rename").unwrap();
        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Renamed".to_string())
        );
    }

    #[test]
    fn encoding_is_deterministic_for_the_same_committed_image() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        app.run_action("createFolder").unwrap();
        let image = app.provider.image.clone().unwrap();

        assert_eq!(image.encode().unwrap(), image.encode().unwrap());
        assert_eq!(
            PersistenceImage::decode(&image.encode().unwrap()).unwrap(),
            image
        );
    }

    #[test]
    fn malformed_bytes_fail_during_decode_before_runtime_restore() {
        let error = PersistenceImage::decode(b"not-an-elanu-persistence-image")
            .expect_err("bad magic must fail during decoding");
        assert!(error.message.contains("magic"));

        let truncated = PERSISTENCE_ENCODING_MAGIC.to_vec();
        let error = PersistenceImage::decode(&truncated)
            .expect_err("missing version/body must fail during decoding");
        assert!(error.message.contains("truncated"));
    }

    #[test]
    fn encoding_version_failure_is_distinct_from_application_shape_mismatch() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        app.run_action("createFolder").unwrap();
        let image = app.provider.image.clone().unwrap();
        let mut bytes = image.encode().unwrap();
        let version_offset = PERSISTENCE_ENCODING_MAGIC.len();
        bytes[version_offset..version_offset + 4].copy_from_slice(&999_u32.to_le_bytes());

        let version_error = PersistenceImage::decode(&bytes)
            .expect_err("unsupported encoding version must fail during decode");
        assert!(version_error.message.contains("encoding version"));

        let incompatible_source = SOURCE.replace(
            "state name = \"\"",
            "state name = \"\"\n    state archived = false",
        );
        let incompatible = check_source_with_runtime_models(&incompatible_source).unwrap();
        let provider = ByteMemoryProvider {
            bytes: Some(image.encode().unwrap()),
            replace_attempts: 0,
        };
        let shape_error = PersistentRuntime::open(incompatible, provider)
            .err()
            .expect("decoded image with incompatible application shape must fail");
        assert!(shape_error.message.contains("checked application shape"));
        assert!(!shape_error.message.contains("encoding version"));
    }

    #[test]
    fn action_body_change_remains_compatible_after_byte_decode() {
        let mut first = PersistentRuntime::open(checked(), ByteMemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        let provider = first.into_provider();

        let changed_source = SOURCE.replace("\"Renamed\"", "\"Renamed after byte restart\"");
        let changed = check_source_with_runtime_models(&changed_source).unwrap();
        let mut restarted = PersistentRuntime::open(changed, provider)
            .expect("action-only change should remain compatible after decode");
        restarted.run_action("rename").unwrap();
        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Renamed after byte restart".to_string())
        );
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
