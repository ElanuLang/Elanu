use super::*;
use crate::check_source_with_runtime_models;
use std::collections::{HashMap, HashSet};

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

derived activeName = activeFolder.name
derived coldName = coldFolder.name
derived coldDocumentTitle = coldFolder.documents[0].title
derived coldInWorkspace = coldFolder is in workspace.folders

action seed {
    create Folder in workspace as active {
        through active.name = "Active"
        insert active into workspace.folders
    }
    activeFolder = workspace.folders[0]
    create Document in activeFolder as activeDocument {
        through activeDocument.title = "Active document"
        insert activeDocument into activeFolder.documents
    }

    create Folder in workspace as cold {
        through cold.name = "Cold"
        insert cold into workspace.folders
    }
    coldFolder = workspace.folders[1]
    create Document in coldFolder as coldDocument {
        through coldDocument.title = "Cold document"
        insert coldDocument into coldFolder.documents
    }
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

const MANIFEST_MAGIC: &[u8; 8] = b"ELANUDBC";
const BACKING_MAGIC: &[u8; 8] = b"ELANUDBK";
const EXPERIMENT_VERSION: u32 = 1;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE)
        .expect("changed dormant-backing pressure source should check")
}

fn cold_subtree_identities(image: &PersistenceImage) -> HashSet<String> {
    let cold_folder = image
        .dynamic_models
        .iter()
        .find_map(|(identity, dynamic)| {
            if dynamic.model_name != "Folder" {
                return None;
            }
            match image
                .state_values
                .get(&model_binding_name(identity, "name"))
            {
                Some(Value::String(value)) if value == "Cold" => Some(identity.clone()),
                _ => None,
            }
        })
        .expect("Cold folder identity should exist");

    let mut subtree = HashSet::from([cold_folder]);
    loop {
        let before = subtree.len();
        for (identity, dynamic) in &image.dynamic_models {
            if subtree.contains(&dynamic.owner) {
                subtree.insert(identity.clone());
            }
        }
        if subtree.len() == before {
            break;
        }
    }
    subtree
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct BackingToken(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
struct BackingHandle {
    model_name: String,
    owner: String,
    token: BackingToken,
}

trait ChangedBackingProvider {
    fn load_manifest(&mut self) -> Result<Vec<u8>, RuntimeError>;
    fn load_backing(&mut self, token: BackingToken) -> Result<Option<Vec<u8>>, RuntimeError>;
    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        backing_replacements: &[(BackingToken, Vec<u8>)],
    ) -> Result<(), RuntimeError>;
}

#[derive(Debug, Clone)]
struct ChangedBackingMemoryProvider {
    manifest: Vec<u8>,
    backing: HashMap<BackingToken, Vec<u8>>,
    manifest_loads: usize,
    backing_loads: Vec<BackingToken>,
    candidate_replace_attempts: usize,
    accepted_replacement_tokens: Vec<BackingToken>,
    reject_next_candidate: bool,
}

impl ChangedBackingProvider for ChangedBackingMemoryProvider {
    fn load_manifest(&mut self) -> Result<Vec<u8>, RuntimeError> {
        self.manifest_loads += 1;
        Ok(self.manifest.clone())
    }

    fn load_backing(&mut self, token: BackingToken) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.backing_loads.push(token);
        Ok(self.backing.get(&token).cloned())
    }

    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        backing_replacements: &[(BackingToken, Vec<u8>)],
    ) -> Result<(), RuntimeError> {
        self.candidate_replace_attempts += 1;
        if self.reject_next_candidate {
            self.reject_next_candidate = false;
            return Err(RuntimeError::new(
                "provider rejected changed-backing candidate",
            ));
        }

        let mut next_backing = self.backing.clone();
        let mut seen = HashSet::new();
        for (token, payload) in backing_replacements {
            if !seen.insert(*token) {
                return Err(RuntimeError::new(
                    "candidate repeats an opaque backing token replacement",
                ));
            }
            if !next_backing.contains_key(token) {
                return Err(RuntimeError::new(
                    "candidate attempts to replace unknown opaque backing token",
                ));
            }
            next_backing.insert(*token, payload.clone());
        }

        // This assignment pair is the fake provider's one atomic acceptance point.
        // A real provider may implement that guarantee differently; the host still
        // sees only opaque tokens and bytes.
        self.backing = next_backing;
        self.manifest = manifest.to_vec();
        self.accepted_replacement_tokens
            .extend(backing_replacements.iter().map(|(token, _)| *token));
        Ok(())
    }
}

fn encode_manifest(
    partial: &PersistenceImage,
    backed: &HashMap<String, BackingHandle>,
) -> Result<Vec<u8>, RuntimeError> {
    let partial_bytes = partial.encode()?;
    let mut encoder = PersistenceEncoder::default();
    encoder.raw(MANIFEST_MAGIC);
    encoder.u32(EXPERIMENT_VERSION);
    encoder.len(partial_bytes.len())?;
    encoder.raw(&partial_bytes);

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
        encoder.u64(handle.token.0);
    }
    Ok(encoder.finish())
}

fn decode_manifest(
    bytes: &[u8],
) -> Result<(PersistenceImage, HashMap<String, BackingHandle>), RuntimeError> {
    let mut decoder = PersistenceDecoder::new(bytes);
    if decoder.raw(MANIFEST_MAGIC.len())? != MANIFEST_MAGIC {
        return Err(RuntimeError::new("invalid changed-backing manifest magic"));
    }
    let version = decoder.u32()?;
    if version != EXPERIMENT_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported changed-backing manifest version {version}"
        )));
    }

    let partial_len = decoder.len()?;
    let partial = PersistenceImage::decode(decoder.raw(partial_len)?)?;
    let count = decoder.len()?;
    let mut backed = HashMap::new();
    let mut tokens = HashSet::new();
    for _ in 0..count {
        let identity = decoder.string()?;
        let handle = BackingHandle {
            model_name: decoder.string()?,
            owner: decoder.string()?,
            token: BackingToken(decoder.u64()?),
        };
        if !tokens.insert(handle.token) {
            return Err(RuntimeError::new(
                "changed-backing manifest repeats an opaque token",
            ));
        }
        if backed.insert(identity.clone(), handle).is_some() {
            return Err(RuntimeError::new(format!(
                "changed-backing manifest repeats identity '{identity}'"
            )));
        }
    }
    decoder.finish()?;
    Ok((partial, backed))
}

fn stored_members<'a>(
    checked: &'a crate::CheckedSource,
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

fn encode_backing_values(
    checked: &crate::CheckedSource,
    identity: &str,
    handle: &BackingHandle,
    values: &HashMap<String, Value>,
) -> Result<Vec<u8>, RuntimeError> {
    let state_members = stored_members(checked, handle)?;
    let mut encoder = PersistenceEncoder::default();
    encoder.raw(BACKING_MAGIC);
    encoder.u32(EXPERIMENT_VERSION);
    encoder.u64(handle.token.0);
    encoder.string(identity)?;
    encoder.string(&handle.model_name)?;
    encoder.len(state_members.len())?;
    for member in state_members {
        let value = values.get(&member.name).ok_or_else(|| {
            RuntimeError::new(format!(
                "backing values are missing '{identity}.{}'",
                member.name
            ))
        })?;
        encode_value(&mut encoder, value)?;
    }
    Ok(encoder.finish())
}

fn backing_values_from_image(
    checked: &crate::CheckedSource,
    image: &PersistenceImage,
    identity: &str,
    handle: &BackingHandle,
) -> Result<HashMap<String, Value>, RuntimeError> {
    let mut values = HashMap::new();
    for member in stored_members(checked, handle)? {
        let name = model_binding_name(identity, &member.name);
        let value = image.state_values.get(&name).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "complete image is missing backing state '{identity}.{}'",
                member.name
            ))
        })?;
        values.insert(member.name.clone(), value);
    }
    Ok(values)
}

fn backing_values_from_runtime(
    checked: &crate::CheckedSource,
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

fn decode_backing(
    checked: &crate::CheckedSource,
    identity: &str,
    handle: &BackingHandle,
    bytes: &[u8],
) -> Result<HashMap<String, Value>, RuntimeError> {
    let mut decoder = PersistenceDecoder::new(bytes);
    if decoder.raw(BACKING_MAGIC.len())? != BACKING_MAGIC {
        return Err(RuntimeError::new("invalid changed-backing payload magic"));
    }
    let version = decoder.u32()?;
    if version != EXPERIMENT_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported changed-backing payload version {version}"
        )));
    }
    if BackingToken(decoder.u64()?) != handle.token {
        return Err(RuntimeError::new(
            "changed-backing payload token does not match request",
        ));
    }
    if decoder.string()? != identity {
        return Err(RuntimeError::new(
            "changed-backing payload identity does not match request",
        ));
    }
    if decoder.string()? != handle.model_name {
        return Err(RuntimeError::new(
            "changed-backing payload model does not match known identity",
        ));
    }

    let state_members = stored_members(checked, handle)?;
    let count = decoder.len()?;
    if count != state_members.len() {
        return Err(RuntimeError::new(format!(
            "changed-backing payload for '{identity}' has {count} states; model '{}' requires {}",
            handle.model_name,
            state_members.len()
        )));
    }

    let mut values = HashMap::new();
    for member in state_members {
        let value = coerce_value(decode_value(&mut decoder)?, &member.value_type)?;
        values.insert(member.name.clone(), value);
    }
    decoder.finish()?;
    Ok(values)
}

fn split_world(
    checked: &crate::CheckedSource,
    image: &PersistenceImage,
    backed_identities: &HashSet<String>,
) -> Result<ChangedBackingMemoryProvider, RuntimeError> {
    let mut partial = image.clone();
    let mut backed = HashMap::new();
    let mut backing = HashMap::new();
    let mut identities = backed_identities.iter().cloned().collect::<Vec<_>>();
    identities.sort();

    for (index, identity) in identities.into_iter().enumerate() {
        let dynamic = image
            .dynamic_models
            .get(&identity)
            .ok_or_else(|| RuntimeError::new(format!("unknown backed identity '{identity}'")))?;
        let token = BackingToken(
            u64::try_from(index)
                .map_err(|_| RuntimeError::new("too many backed identities for experiment"))?,
        );
        let handle = BackingHandle {
            model_name: dynamic.model_name.clone(),
            owner: dynamic.owner.clone(),
            token,
        };
        let values = backing_values_from_image(checked, image, &identity, &handle)?;
        let payload = encode_backing_values(checked, &identity, &handle, &values)?;
        backing.insert(token, payload);

        for member in stored_members(checked, &handle)? {
            partial
                .state_values
                .remove(&model_binding_name(&identity, &member.name));
        }
        partial.dynamic_models.remove(&identity);
        backed.insert(identity, handle);
    }

    Ok(ChangedBackingMemoryProvider {
        manifest: encode_manifest(&partial, &backed)?,
        backing,
        manifest_loads: 0,
        backing_loads: Vec::new(),
        candidate_replace_attempts: 0,
        accepted_replacement_tokens: Vec::new(),
        reject_next_candidate: false,
    })
}

fn validate_backed_metadata(
    runtime: &Runtime,
    backed: &HashMap<String, BackingHandle>,
) -> Result<(), RuntimeError> {
    for (identity, handle) in backed {
        if runtime.dynamic_model_types.get(identity) != Some(&handle.model_name) {
            return Err(RuntimeError::new(format!(
                "backed identity '{identity}' changed model type"
            )));
        }
        if runtime.dynamic_model_owners.get(identity) != Some(&handle.owner) {
            return Err(RuntimeError::new(format!(
                "backed identity '{identity}' changed lifetime owner"
            )));
        }
    }
    Ok(())
}

fn capture_partial_image(
    checked: &crate::CheckedSource,
    runtime: &Runtime,
    shape: &PersistenceShape,
    backed: &HashMap<String, BackingHandle>,
) -> Result<PersistenceImage, RuntimeError> {
    validate_backed_metadata(runtime, backed)?;
    let mut partial = capture_image(runtime, shape)?;
    for (identity, handle) in backed {
        partial.dynamic_models.remove(identity);
        for member in stored_members(checked, handle)? {
            partial
                .state_values
                .remove(&model_binding_name(identity, &member.name));
        }
    }
    Ok(partial)
}

fn restore_partial_runtime(
    checked: &crate::CheckedSource,
    shape: &PersistenceShape,
    partial: &PersistenceImage,
    backed: &HashMap<String, BackingHandle>,
) -> Result<Runtime, RuntimeError> {
    let mut runtime = Runtime::from_checked_source(checked)?;
    restore_image(&mut runtime, shape, partial)?;

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
    Ok(runtime)
}

fn install_backed_values(
    checked: &crate::CheckedSource,
    runtime: &mut Runtime,
    identity: &str,
    handle: &BackingHandle,
    values: &HashMap<String, Value>,
) -> Result<(), RuntimeError> {
    if runtime.dynamic_model_types.get(identity) != Some(&handle.model_name) {
        return Err(RuntimeError::new(format!(
            "backed identity '{identity}' changed model type"
        )));
    }
    if runtime.dynamic_model_owners.get(identity) != Some(&handle.owner) {
        return Err(RuntimeError::new(format!(
            "backed identity '{identity}' changed lifetime owner"
        )));
    }

    let template = checked
        .runtime_model_templates
        .get(&handle.model_name)
        .cloned()
        .expect("backing model should have been validated");
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
                    value: values.get(&member.name).cloned().ok_or_else(|| {
                        RuntimeError::new(format!(
                            "validated backing is missing '{identity}.{}'",
                            member.name
                        ))
                    })?,
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

fn capture_materialized_values(
    checked: &crate::CheckedSource,
    runtime: &Runtime,
    backed: &HashMap<String, BackingHandle>,
    materialized: &HashSet<String>,
) -> Result<HashMap<String, HashMap<String, Value>>, RuntimeError> {
    let mut snapshots = HashMap::new();
    for identity in materialized {
        let handle = backed.get(identity).ok_or_else(|| {
            RuntimeError::new(format!(
                "materialized identity '{identity}' has no backing handle"
            ))
        })?;
        snapshots.insert(
            identity.clone(),
            backing_values_from_runtime(checked, runtime, identity, handle)?,
        );
    }
    Ok(snapshots)
}

struct ChangedBackingRuntime<P> {
    checked: crate::CheckedSource,
    shape: PersistenceShape,
    runtime: Runtime,
    backed: HashMap<String, BackingHandle>,
    materialized: HashSet<String>,
    provider: P,
}

impl<P: ChangedBackingProvider> ChangedBackingRuntime<P> {
    fn open(checked: crate::CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let shape = persistence_shape(&checked)?;
        let manifest = provider.load_manifest()?;
        let (partial, backed) = decode_manifest(&manifest)?;
        let runtime = restore_partial_runtime(&checked, &shape, &partial, &backed)?;
        Ok(Self {
            checked,
            shape,
            runtime,
            backed,
            materialized: HashSet::new(),
            provider,
        })
    }

    fn materialize(&mut self, identity: &str) -> Result<(), RuntimeError> {
        if self.materialized.contains(identity) {
            return Ok(());
        }
        let handle = self
            .backed
            .get(identity)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("identity '{identity}' is not backed")))?;
        let payload = self.provider.load_backing(handle.token)?.ok_or_else(|| {
            RuntimeError::new(format!("missing backing payload for '{identity}'"))
        })?;
        let values = decode_backing(&self.checked, identity, &handle, &payload)?;
        install_backed_values(&self.checked, &mut self.runtime, identity, &handle, &values)?;
        self.materialized.insert(identity.to_string());
        Ok(())
    }

    fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        if self.runtime.transaction.is_some() {
            return Err(RuntimeError::new(
                "changed-backing runtime cannot start an action while a transaction is active",
            ));
        }

        let prior_next_dynamic_identity = self.runtime.next_dynamic_identity;
        let prior_partial =
            capture_partial_image(&self.checked, &self.runtime, &self.shape, &self.backed)?;
        let prior_materialized = capture_materialized_values(
            &self.checked,
            &self.runtime,
            &self.backed,
            &self.materialized,
        )?;

        self.runtime.transaction = Some(Transaction::default());
        let result = self.runtime.invoke_action(name, &[]);
        let transaction = match result {
            Ok(()) => self
                .runtime
                .transaction
                .take()
                .expect("successful action should retain its transaction"),
            Err(error) => {
                self.runtime.transaction = None;
                self.runtime.next_dynamic_identity = prior_next_dynamic_identity;
                return Err(error);
            }
        };

        let candidate_next_dynamic_identity = self.runtime.next_dynamic_identity;
        let mut candidate =
            restore_partial_runtime(&self.checked, &self.shape, &prior_partial, &self.backed)?;
        for identity in &self.materialized {
            let handle = self
                .backed
                .get(identity)
                .expect("materialized identity should retain its backing handle");
            let values = prior_materialized
                .get(identity)
                .expect("materialized identity should have a prior value snapshot");
            install_backed_values(&self.checked, &mut candidate, identity, handle, values)?;
        }
        candidate.next_dynamic_identity = candidate_next_dynamic_identity;
        candidate.commit(transaction);

        let candidate_partial =
            capture_partial_image(&self.checked, &candidate, &self.shape, &self.backed)?;
        let candidate_manifest = encode_manifest(&candidate_partial, &self.backed)?;

        let mut replacements = Vec::new();
        let mut materialized = self.materialized.iter().collect::<Vec<_>>();
        materialized.sort();
        for identity in materialized {
            let handle = self
                .backed
                .get(identity)
                .expect("materialized identity should retain its backing handle");
            let prior_values = prior_materialized
                .get(identity)
                .expect("materialized identity should have a prior value snapshot");
            let candidate_values =
                backing_values_from_runtime(&self.checked, &candidate, identity, handle)?;
            let prior_payload =
                encode_backing_values(&self.checked, identity, handle, prior_values)?;
            let candidate_payload =
                encode_backing_values(&self.checked, identity, handle, &candidate_values)?;
            if candidate_payload != prior_payload {
                replacements.push((handle.token, candidate_payload));
            }
        }

        if let Err(error) = self
            .provider
            .replace_candidate(&candidate_manifest, &replacements)
        {
            self.runtime.next_dynamic_identity = prior_next_dynamic_identity;
            return Err(error);
        }

        self.runtime = candidate;
        Ok(())
    }

    fn value(&mut self, name: &str) -> Result<Value, RuntimeError> {
        self.runtime.value(name)
    }

    fn into_provider(self) -> P {
        self.provider
    }
}

fn seeded_store() -> (
    crate::CheckedSource,
    PersistenceImage,
    HashSet<String>,
    ChangedBackingMemoryProvider,
) {
    let checked = checked_source();
    let shape = persistence_shape(&checked).expect("persistence shape should build");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    runtime.run_action("seed").expect("seed should commit");
    let image = capture_image(&runtime, &shape).expect("seeded world should capture");
    let cold = cold_subtree_identities(&image);
    assert_eq!(cold.len(), 2, "Cold Folder + Cold Document");
    let provider = split_world(&checked, &image, &cold).expect("world split should succeed");
    (checked, image, cold, provider)
}

fn cold_identities(image: &PersistenceImage, cold: &HashSet<String>) -> (String, String) {
    let folder = cold
        .iter()
        .find(|identity| image.dynamic_models[*identity].model_name == "Folder")
        .cloned()
        .expect("Cold Folder identity");
    let document = cold
        .iter()
        .find(|identity| image.dynamic_models[*identity].model_name == "Document")
        .cloned()
        .expect("Cold Document identity");
    (folder, document)
}

#[test]
fn changed_materialized_folder_replaces_only_its_opaque_backing() {
    let (checked, image, cold, provider) = seeded_store();
    let (cold_folder, cold_document) = cold_identities(&image, &cold);
    let (_, handles) = decode_manifest(&provider.manifest).unwrap();
    let folder_token = handles[&cold_folder].token;
    let document_token = handles[&cold_document].token;
    let manifest_before = provider.manifest.clone();
    let folder_backing_before = provider.backing[&folder_token].clone();
    let document_backing_before = provider.backing[&document_token].clone();
    let original_types = image
        .dynamic_models
        .iter()
        .map(|(identity, dynamic)| (identity.clone(), dynamic.model_name.clone()))
        .collect::<HashMap<_, _>>();
    let original_owners = image
        .dynamic_models
        .iter()
        .map(|(identity, dynamic)| (identity.clone(), dynamic.owner.clone()))
        .collect::<HashMap<_, _>>();
    let allocator = image.next_dynamic_identity;

    let mut app = ChangedBackingRuntime::open(checked.clone(), provider).unwrap();
    assert!(app.provider.backing_loads.is_empty());
    assert_eq!(
        app.value("audit").unwrap(),
        Value::String("idle".to_string())
    );
    assert_eq!(app.value("coldInWorkspace").unwrap(), Value::Bool(true));

    app.materialize(&cold_folder)
        .expect("explicit Folder materialization should load only Folder backing");
    assert_eq!(app.provider.backing_loads, vec![folder_token]);
    assert_eq!(
        app.value("coldName").unwrap(),
        Value::String("Cold".to_string())
    );
    app.value("coldDocumentTitle")
        .expect_err("Cold Document must remain dormant");
    assert_eq!(app.provider.backing_loads, vec![folder_token]);

    app.run_action("renameCold")
        .expect("changed Folder + resident audit state should publish atomically");

    assert_eq!(
        app.value("coldName").unwrap(),
        Value::String("Cold renamed".to_string())
    );
    assert_eq!(
        app.value("audit").unwrap(),
        Value::String("renamed cold".to_string())
    );
    assert_eq!(app.provider.candidate_replace_attempts, 1);
    assert_eq!(app.provider.accepted_replacement_tokens, vec![folder_token]);
    assert_ne!(app.provider.manifest, manifest_before);
    assert_ne!(app.provider.backing[&folder_token], folder_backing_before);
    assert_eq!(
        app.provider.backing[&document_token],
        document_backing_before
    );
    assert_eq!(app.provider.backing_loads, vec![folder_token]);
    assert_eq!(app.runtime.dynamic_model_types, original_types);
    assert_eq!(app.runtime.dynamic_model_owners, original_owners);
    assert_eq!(app.runtime.next_dynamic_identity, allocator);

    let mut provider = app.into_provider();
    provider.backing_loads.clear();
    let accepted_folder_backing = provider.backing[&folder_token].clone();
    let mut restarted = ChangedBackingRuntime::open(checked, provider).unwrap();
    assert!(restarted.provider.backing_loads.is_empty());
    assert_eq!(
        restarted.value("audit").unwrap(),
        Value::String("renamed cold".to_string())
    );
    restarted
        .value("coldName")
        .expect_err("Folder should restart dormant despite changed durable backing");
    assert!(restarted.provider.backing_loads.is_empty());

    restarted.materialize(&cold_folder).unwrap();
    assert_eq!(restarted.provider.backing_loads, vec![folder_token]);
    assert_eq!(
        restarted.value("coldName").unwrap(),
        Value::String("Cold renamed".to_string())
    );
    assert_eq!(
        restarted.provider.backing[&folder_token],
        accepted_folder_backing
    );
    assert_eq!(
        restarted.provider.backing[&document_token],
        document_backing_before
    );
    restarted
        .value("coldDocumentTitle")
        .expect_err("Document should remain independently dormant after restart");
    assert_eq!(restarted.provider.backing_loads, vec![folder_token]);
}

#[test]
fn provider_rejection_preserves_manifest_all_backing_and_authoritative_runtime() {
    let (checked, image, cold, provider) = seeded_store();
    let (cold_folder, cold_document) = cold_identities(&image, &cold);
    let (_, handles) = decode_manifest(&provider.manifest).unwrap();
    let folder_token = handles[&cold_folder].token;
    let document_token = handles[&cold_document].token;
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();

    let mut app = ChangedBackingRuntime::open(checked.clone(), provider).unwrap();
    app.materialize(&cold_folder).unwrap();
    assert_eq!(app.provider.backing_loads, vec![folder_token]);
    app.provider.reject_next_candidate = true;

    let error = app
        .run_action("renameCold")
        .expect_err("provider rejection must reject manifest + changed backing together");
    assert!(error.message.contains("rejected changed-backing candidate"));
    assert_eq!(app.provider.candidate_replace_attempts, 1);
    assert!(app.provider.accepted_replacement_tokens.is_empty());
    assert_eq!(app.provider.manifest, manifest_before);
    assert_eq!(app.provider.backing, backing_before);
    assert_eq!(
        app.provider.backing[&document_token],
        backing_before[&document_token]
    );
    assert_eq!(
        app.value("coldName").unwrap(),
        Value::String("Cold".to_string())
    );
    assert_eq!(
        app.value("audit").unwrap(),
        Value::String("idle".to_string())
    );

    let provider = app.into_provider();
    let mut restarted = ChangedBackingRuntime::open(checked, provider).unwrap();
    assert!(restarted
        .provider
        .backing_loads
        .iter()
        .all(|token| *token == folder_token));
    restarted.provider.backing_loads.clear();
    assert_eq!(
        restarted.value("audit").unwrap(),
        Value::String("idle".to_string())
    );
    restarted.materialize(&cold_folder).unwrap();
    assert_eq!(
        restarted.value("coldName").unwrap(),
        Value::String("Cold".to_string())
    );
    assert_eq!(
        restarted.provider.backing[&document_token],
        backing_before[&document_token]
    );
}

#[test]
fn semantic_failure_does_not_attempt_changed_backing_publication() {
    let (checked, image, cold, provider) = seeded_store();
    let (cold_folder, _cold_document) = cold_identities(&image, &cold);
    let (_, handles) = decode_manifest(&provider.manifest).unwrap();
    let folder_token = handles[&cold_folder].token;
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();

    let mut app = ChangedBackingRuntime::open(checked, provider).unwrap();
    app.materialize(&cold_folder).unwrap();
    assert_eq!(app.provider.backing_loads, vec![folder_token]);

    let error = app
        .run_action("failedRenameCold")
        .expect_err("semantic failure should stop before provider publication");
    assert!(error.message.contains("abort"));
    assert_eq!(app.provider.candidate_replace_attempts, 0);
    assert!(app.provider.accepted_replacement_tokens.is_empty());
    assert_eq!(app.provider.manifest, manifest_before);
    assert_eq!(app.provider.backing, backing_before);
    assert_eq!(
        app.value("coldName").unwrap(),
        Value::String("Cold".to_string())
    );
    assert_eq!(
        app.value("audit").unwrap(),
        Value::String("idle".to_string())
    );
}
