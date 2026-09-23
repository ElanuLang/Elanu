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

derived activeName = activeFolder.name
derived coldName = coldFolder.name
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

action renameActive {
    through activeFolder.name = "Active renamed"
}

action failedRenameActive {
    through activeFolder.name = "Wrong"
    fail "abort"
}
"#;

const MANIFEST_MAGIC: &[u8; 8] = b"ELANUINC";
const BACKING_MAGIC: &[u8; 8] = b"ELANUCHK";
const EXPERIMENT_VERSION: u32 = 1;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE)
        .expect("incremental dormant-publication pressure source should check")
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
struct DormantHandle {
    model_name: String,
    owner: String,
    token: BackingToken,
}

trait IncrementalPublicationProvider {
    fn load_manifest(&mut self) -> Result<Vec<u8>, RuntimeError>;
    fn replace_manifest(&mut self, manifest: &[u8]) -> Result<(), RuntimeError>;
}

#[derive(Debug, Clone)]
struct IncrementalMemoryProvider {
    manifest: Vec<u8>,
    backing: HashMap<BackingToken, Vec<u8>>,
    manifest_loads: usize,
    manifest_replace_attempts: usize,
    backing_replacements: usize,
    reject_next_manifest_replace: bool,
}

impl IncrementalPublicationProvider for IncrementalMemoryProvider {
    fn load_manifest(&mut self) -> Result<Vec<u8>, RuntimeError> {
        self.manifest_loads += 1;
        Ok(self.manifest.clone())
    }

    fn replace_manifest(&mut self, manifest: &[u8]) -> Result<(), RuntimeError> {
        self.manifest_replace_attempts += 1;
        if self.reject_next_manifest_replace {
            self.reject_next_manifest_replace = false;
            return Err(RuntimeError::new("provider rejected candidate manifest"));
        }
        self.manifest = manifest.to_vec();
        Ok(())
    }
}

fn encode_manifest(
    partial: &PersistenceImage,
    dormant: &HashMap<String, DormantHandle>,
) -> Result<Vec<u8>, RuntimeError> {
    let partial_bytes = partial.encode()?;
    let mut encoder = PersistenceEncoder::default();
    encoder.raw(MANIFEST_MAGIC);
    encoder.u32(EXPERIMENT_VERSION);
    encoder.len(partial_bytes.len())?;
    encoder.raw(&partial_bytes);

    let mut identities = dormant.keys().collect::<Vec<_>>();
    identities.sort();
    encoder.len(identities.len())?;
    for identity in identities {
        let handle = dormant
            .get(identity)
            .expect("sorted dormant identity should still exist");
        encoder.string(identity)?;
        encoder.string(&handle.model_name)?;
        encoder.string(&handle.owner)?;
        encoder.u64(handle.token.0);
    }
    Ok(encoder.finish())
}

fn decode_manifest(
    bytes: &[u8],
) -> Result<(PersistenceImage, HashMap<String, DormantHandle>), RuntimeError> {
    let mut decoder = PersistenceDecoder::new(bytes);
    if decoder.raw(MANIFEST_MAGIC.len())? != MANIFEST_MAGIC {
        return Err(RuntimeError::new("invalid incremental manifest magic"));
    }
    let version = decoder.u32()?;
    if version != EXPERIMENT_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported incremental manifest version {version}"
        )));
    }

    let partial_len = decoder.len()?;
    let partial = PersistenceImage::decode(decoder.raw(partial_len)?)?;
    let count = decoder.len()?;
    let mut dormant = HashMap::new();
    let mut tokens = HashSet::new();
    for _ in 0..count {
        let identity = decoder.string()?;
        let handle = DormantHandle {
            model_name: decoder.string()?,
            owner: decoder.string()?,
            token: BackingToken(decoder.u64()?),
        };
        if !tokens.insert(handle.token) {
            return Err(RuntimeError::new(
                "incremental manifest repeats a backing token",
            ));
        }
        if dormant.insert(identity.clone(), handle).is_some() {
            return Err(RuntimeError::new(format!(
                "incremental manifest repeats identity '{identity}'"
            )));
        }
    }
    decoder.finish()?;
    Ok((partial, dormant))
}

fn encode_backing(
    checked: &crate::CheckedSource,
    image: &PersistenceImage,
    identity: &str,
    handle: &DormantHandle,
) -> Result<Vec<u8>, RuntimeError> {
    let template = checked
        .runtime_model_templates
        .get(&handle.model_name)
        .ok_or_else(|| RuntimeError::new(format!("unknown model '{}'", handle.model_name)))?;

    let mut encoder = PersistenceEncoder::default();
    encoder.raw(BACKING_MAGIC);
    encoder.u32(EXPERIMENT_VERSION);
    encoder.u64(handle.token.0);
    encoder.string(identity)?;
    encoder.string(&handle.model_name)?;

    let state_members = template
        .members
        .iter()
        .filter(|member| member.kind == RuntimeModelMemberKind::State)
        .collect::<Vec<_>>();
    encoder.len(state_members.len())?;
    for member in state_members {
        let name = model_binding_name(identity, &member.name);
        let value = image.state_values.get(&name).ok_or_else(|| {
            RuntimeError::new(format!(
                "complete image is missing dormant state '{identity}.{}'",
                member.name
            ))
        })?;
        encode_value(&mut encoder, value)?;
    }
    Ok(encoder.finish())
}

fn split_world(
    checked: &crate::CheckedSource,
    image: &PersistenceImage,
    dormant_identities: &HashSet<String>,
) -> Result<IncrementalMemoryProvider, RuntimeError> {
    let mut partial = image.clone();
    let mut dormant = HashMap::new();
    let mut backing = HashMap::new();
    let mut identities = dormant_identities.iter().cloned().collect::<Vec<_>>();
    identities.sort();

    for (index, identity) in identities.into_iter().enumerate() {
        let dynamic = image
            .dynamic_models
            .get(&identity)
            .ok_or_else(|| RuntimeError::new(format!("unknown dormant identity '{identity}'")))?;
        let token = BackingToken(
            u64::try_from(index)
                .map_err(|_| RuntimeError::new("too many dormant identities for experiment"))?,
        );
        let handle = DormantHandle {
            model_name: dynamic.model_name.clone(),
            owner: dynamic.owner.clone(),
            token,
        };
        let payload = encode_backing(checked, image, &identity, &handle)?;
        if backing.insert(token, payload).is_some() {
            return Err(RuntimeError::new("duplicate dormant backing token"));
        }

        let template = checked
            .runtime_model_templates
            .get(&dynamic.model_name)
            .expect("known dynamic model should have a checked template");
        for member in template
            .members
            .iter()
            .filter(|member| member.kind == RuntimeModelMemberKind::State)
        {
            partial
                .state_values
                .remove(&model_binding_name(&identity, &member.name));
        }
        partial.dynamic_models.remove(&identity);
        dormant.insert(identity, handle);
    }

    Ok(IncrementalMemoryProvider {
        manifest: encode_manifest(&partial, &dormant)?,
        backing,
        manifest_loads: 0,
        manifest_replace_attempts: 0,
        backing_replacements: 0,
        reject_next_manifest_replace: false,
    })
}

fn validate_dormant_metadata(
    runtime: &Runtime,
    dormant: &HashMap<String, DormantHandle>,
) -> Result<(), RuntimeError> {
    for (identity, handle) in dormant {
        if runtime.dynamic_model_types.get(identity) != Some(&handle.model_name) {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' changed model type"
            )));
        }
        if runtime.dynamic_model_owners.get(identity) != Some(&handle.owner) {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' changed lifetime owner"
            )));
        }
    }
    Ok(())
}

fn capture_partial_image(
    runtime: &Runtime,
    shape: &PersistenceShape,
    dormant: &HashMap<String, DormantHandle>,
) -> Result<PersistenceImage, RuntimeError> {
    validate_dormant_metadata(runtime, dormant)?;
    let mut partial = capture_image(runtime, shape)?;
    for identity in dormant.keys() {
        partial.dynamic_models.remove(identity);
    }
    Ok(partial)
}

fn restore_partial_runtime(
    checked: &crate::CheckedSource,
    shape: &PersistenceShape,
    partial: &PersistenceImage,
    dormant: &HashMap<String, DormantHandle>,
) -> Result<Runtime, RuntimeError> {
    let mut runtime = Runtime::from_checked_source(checked)?;
    restore_image(&mut runtime, shape, partial)?;

    for (identity, handle) in dormant {
        if runtime.dynamic_model_types.contains_key(identity)
            || runtime.dynamic_model_owners.contains_key(identity)
        {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' duplicates a resident identity"
            )));
        }
        if !checked
            .runtime_model_templates
            .contains_key(&handle.model_name)
        {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' references unknown model '{}'",
                handle.model_name
            )));
        }
        let owner_exists = runtime.runtime_model_roots.contains_key(&handle.owner)
            || runtime.dynamic_model_types.contains_key(&handle.owner)
            || dormant.contains_key(&handle.owner);
        if !owner_exists {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' references unknown owner '{}'",
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

struct IncrementalBackedRuntime<P> {
    checked: crate::CheckedSource,
    shape: PersistenceShape,
    runtime: Runtime,
    dormant: HashMap<String, DormantHandle>,
    provider: P,
}

impl<P: IncrementalPublicationProvider> IncrementalBackedRuntime<P> {
    fn open(checked: crate::CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let shape = persistence_shape(&checked)?;
        let manifest = provider.load_manifest()?;
        let (partial, dormant) = decode_manifest(&manifest)?;
        let runtime = restore_partial_runtime(&checked, &shape, &partial, &dormant)?;
        Ok(Self {
            checked,
            shape,
            runtime,
            dormant,
            provider,
        })
    }

    fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        if self.runtime.transaction.is_some() {
            return Err(RuntimeError::new(
                "incremental runtime cannot start an action while a transaction is active",
            ));
        }

        let prior_next_dynamic_identity = self.runtime.next_dynamic_identity;
        let prior_partial = capture_partial_image(&self.runtime, &self.shape, &self.dormant)?;
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
        let mut candidate = restore_partial_runtime(
            &self.checked,
            &self.shape,
            &prior_partial,
            &self.dormant,
        )?;
        candidate.next_dynamic_identity = candidate_next_dynamic_identity;
        candidate.commit(transaction);
        let candidate_partial = capture_partial_image(&candidate, &self.shape, &self.dormant)?;
        let candidate_manifest = encode_manifest(&candidate_partial, &self.dormant)?;

        if let Err(error) = self.provider.replace_manifest(&candidate_manifest) {
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
    IncrementalMemoryProvider,
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

#[test]
fn resident_only_candidate_reuses_all_dormant_backing_without_loading_or_rewriting() {
    let (checked, image, cold, provider) = seeded_store();
    let backing_before = provider.backing.clone();
    let (_, handles_before) = decode_manifest(&provider.manifest).unwrap();
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

    let mut app = IncrementalBackedRuntime::open(checked.clone(), provider).unwrap();
    assert_eq!(app.provider.manifest_loads, 1);
    assert_eq!(app.provider.manifest_replace_attempts, 0);
    app.run_action("renameActive")
        .expect("resident-only action should publish incrementally");

    assert_eq!(
        app.value("activeName").unwrap(),
        Value::String("Active renamed".to_string())
    );
    assert_eq!(app.provider.manifest_replace_attempts, 1);
    assert_eq!(app.provider.backing_replacements, 0);
    assert_eq!(app.provider.backing, backing_before);
    let (_, handles_after) = decode_manifest(&app.provider.manifest).unwrap();
    assert_eq!(handles_after, handles_before);
    assert_eq!(app.runtime.dynamic_model_types, original_types);
    assert_eq!(app.runtime.dynamic_model_owners, original_owners);
    assert_eq!(app.runtime.next_dynamic_identity, allocator);

    let mut provider = app.into_provider();
    provider.manifest_loads = 0;
    let backing_after_acceptance = provider.backing.clone();
    let mut restarted = IncrementalBackedRuntime::open(checked, provider).unwrap();
    assert_eq!(restarted.provider.manifest_loads, 1);
    assert_eq!(
        restarted.value("activeName").unwrap(),
        Value::String("Active renamed".to_string())
    );
    assert_eq!(
        restarted.value("coldInWorkspace").unwrap(),
        Value::Bool(true)
    );
    restarted
        .value("coldName")
        .expect_err("Cold should remain dormant after accepted restart");
    assert_eq!(restarted.provider.backing, backing_after_acceptance);
    assert_eq!(restarted.dormant.len(), cold.len());
}

#[test]
fn provider_rejection_preserves_prior_manifest_backing_and_authoritative_runtime() {
    let (checked, _image, _cold, provider) = seeded_store();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    let mut app = IncrementalBackedRuntime::open(checked.clone(), provider).unwrap();
    let types_before = app.runtime.dynamic_model_types.clone();
    let owners_before = app.runtime.dynamic_model_owners.clone();
    let allocator_before = app.runtime.next_dynamic_identity;
    app.provider.reject_next_manifest_replace = true;

    let error = app
        .run_action("renameActive")
        .expect_err("provider rejection must reject the whole action");
    assert!(error.message.contains("rejected candidate manifest"));
    assert_eq!(app.provider.manifest_replace_attempts, 1);
    assert_eq!(app.provider.manifest, manifest_before);
    assert_eq!(app.provider.backing, backing_before);
    assert_eq!(app.provider.backing_replacements, 0);
    assert_eq!(
        app.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
    assert_eq!(app.runtime.dynamic_model_types, types_before);
    assert_eq!(app.runtime.dynamic_model_owners, owners_before);
    assert_eq!(app.runtime.next_dynamic_identity, allocator_before);

    let provider = app.into_provider();
    let mut restarted = IncrementalBackedRuntime::open(checked, provider).unwrap();
    assert_eq!(
        restarted.value("activeName").unwrap(),
        Value::String("Active".to_string()),
        "restart after rejection must observe the prior durable world"
    );
    assert_eq!(restarted.provider.backing, backing_before);
}

#[test]
fn semantic_failure_never_attempts_incremental_publication() {
    let (checked, _image, _cold, provider) = seeded_store();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    let mut app = IncrementalBackedRuntime::open(checked, provider).unwrap();

    let error = app
        .run_action("failedRenameActive")
        .expect_err("semantic failure should abort before publication");
    assert!(error.message.contains("abort"));
    assert_eq!(app.provider.manifest_replace_attempts, 0);
    assert_eq!(app.provider.manifest, manifest_before);
    assert_eq!(app.provider.backing, backing_before);
    assert_eq!(app.provider.backing_replacements, 0);
    assert_eq!(
        app.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
}
