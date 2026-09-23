use super::*;
use crate::check_source_with_runtime_models;
use std::collections::{HashMap, HashSet};

const SOURCE: &str = r#"
state model Document {
    state title = ""
    derived label = title + " document"
}

state model Folder {
    state name = ""
    state folders: [live Folder] = []
    state documents: [live Document] = []
    derived label = name + " folder"
}

state workspace: Folder
state activeFolder: maybe live Folder = none
state coldFolder: maybe live Folder = none

derived activeName = activeFolder.name
derived coldName = coldFolder.name
derived coldLabel = coldFolder.label
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

action renameActive {
    through activeFolder.name = "Active renamed"
}
"#;

const MANIFEST_MAGIC: &[u8; 8] = b"ELANUDRM";
const BACKING_MAGIC: &[u8; 8] = b"ELANUBKG";
const EXPERIMENT_ENCODING_VERSION: u32 = 1;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE)
        .expect("opaque dormant-backing pressure source should check")
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
struct DormantBackingToken(u64);

#[derive(Debug, Clone)]
struct DormantHandle {
    model_name: String,
    owner: String,
    token: DormantBackingToken,
}

trait OpaqueDormantBackingProvider {
    fn load_manifest(&mut self) -> Result<Vec<u8>, RuntimeError>;
    fn load_backing(&mut self, token: DormantBackingToken)
        -> Result<Option<Vec<u8>>, RuntimeError>;
}

#[derive(Debug, Clone)]
struct OpaqueMemoryBackingProvider {
    manifest_bytes: Vec<u8>,
    backing: HashMap<DormantBackingToken, Vec<u8>>,
    manifest_loads: usize,
    backing_loads: Vec<DormantBackingToken>,
}

impl OpaqueDormantBackingProvider for OpaqueMemoryBackingProvider {
    fn load_manifest(&mut self) -> Result<Vec<u8>, RuntimeError> {
        self.manifest_loads += 1;
        Ok(self.manifest_bytes.clone())
    }

    fn load_backing(
        &mut self,
        token: DormantBackingToken,
    ) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.backing_loads.push(token);
        Ok(self.backing.get(&token).cloned())
    }
}

fn encode_manifest(
    partial: &PersistenceImage,
    dormant: &HashMap<String, DormantHandle>,
) -> Result<Vec<u8>, RuntimeError> {
    let partial_bytes = partial.encode()?;
    let mut encoder = PersistenceEncoder::default();
    encoder.raw(MANIFEST_MAGIC);
    encoder.u32(EXPERIMENT_ENCODING_VERSION);
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
        return Err(RuntimeError::new("invalid dormant manifest magic"));
    }
    let version = decoder.u32()?;
    if version != EXPERIMENT_ENCODING_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported dormant manifest version {version}"
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
            token: DormantBackingToken(decoder.u64()?),
        };
        if !tokens.insert(handle.token) {
            return Err(RuntimeError::new(
                "dormant manifest repeats a backing token",
            ));
        }
        if dormant.insert(identity.clone(), handle).is_some() {
            return Err(RuntimeError::new(format!(
                "dormant manifest repeats identity '{identity}'"
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
    let state_members = template
        .members
        .iter()
        .filter(|member| member.kind == RuntimeModelMemberKind::State)
        .collect::<Vec<_>>();

    let mut encoder = PersistenceEncoder::default();
    encoder.raw(BACKING_MAGIC);
    encoder.u32(EXPERIMENT_ENCODING_VERSION);
    encoder.u64(handle.token.0);
    encoder.string(identity)?;
    encoder.string(&handle.model_name)?;
    encoder.len(state_members.len())?;
    for member in state_members {
        let name = model_binding_name(identity, &member.name);
        let value = image.state_values.get(&name).ok_or_else(|| {
            RuntimeError::new(format!(
                "complete image is missing backing state '{identity}.{}'",
                member.name
            ))
        })?;
        encode_value(&mut encoder, value)?;
    }
    Ok(encoder.finish())
}

fn decode_backing(
    checked: &crate::CheckedSource,
    identity: &str,
    handle: &DormantHandle,
    bytes: &[u8],
) -> Result<HashMap<String, Value>, RuntimeError> {
    let mut decoder = PersistenceDecoder::new(bytes);
    if decoder.raw(BACKING_MAGIC.len())? != BACKING_MAGIC {
        return Err(RuntimeError::new("invalid dormant backing magic"));
    }
    let version = decoder.u32()?;
    if version != EXPERIMENT_ENCODING_VERSION {
        return Err(RuntimeError::new(format!(
            "unsupported dormant backing version {version}"
        )));
    }
    let token = DormantBackingToken(decoder.u64()?);
    if token != handle.token {
        return Err(RuntimeError::new(
            "dormant backing token does not match request",
        ));
    }
    if decoder.string()? != identity {
        return Err(RuntimeError::new(
            "dormant backing identity does not match request",
        ));
    }
    if decoder.string()? != handle.model_name {
        return Err(RuntimeError::new(
            "dormant backing model does not match known identity",
        ));
    }

    let template = checked
        .runtime_model_templates
        .get(&handle.model_name)
        .ok_or_else(|| RuntimeError::new(format!("unknown model '{}'", handle.model_name)))?;
    let state_members = template
        .members
        .iter()
        .filter(|member| member.kind == RuntimeModelMemberKind::State)
        .collect::<Vec<_>>();
    let count = decoder.len()?;
    if count != state_members.len() {
        return Err(RuntimeError::new(format!(
            "dormant backing for '{identity}' has {count} stored states; model '{}' requires {}",
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

fn split_world_into_opaque_backing(
    checked: &crate::CheckedSource,
    image: &PersistenceImage,
    dormant_identities: &HashSet<String>,
) -> Result<OpaqueMemoryBackingProvider, RuntimeError> {
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
        let token = DormantBackingToken(
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
            return Err(RuntimeError::new("duplicate opaque dormant token"));
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

    Ok(OpaqueMemoryBackingProvider {
        manifest_bytes: encode_manifest(&partial, &dormant)?,
        backing,
        manifest_loads: 0,
        backing_loads: Vec::new(),
    })
}

struct OpaqueBackedRuntime<P> {
    checked: crate::CheckedSource,
    shape: PersistenceShape,
    runtime: Runtime,
    dormant: HashMap<String, DormantHandle>,
    provider: P,
}

impl<P: OpaqueDormantBackingProvider> OpaqueBackedRuntime<P> {
    fn open(checked: crate::CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let shape = persistence_shape(&checked)?;
        let manifest_bytes = provider.load_manifest()?;
        let (partial, dormant) = decode_manifest(&manifest_bytes)?;
        let mut runtime = Runtime::from_checked_source(&checked)?;
        restore_image(&mut runtime, &shape, &partial)?;

        for (identity, handle) in &dormant {
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

        Ok(Self {
            checked,
            shape,
            runtime,
            dormant,
            provider,
        })
    }

    fn materialize(&mut self, identity: &str) -> Result<(), RuntimeError> {
        let handle =
            self.dormant.get(identity).cloned().ok_or_else(|| {
                RuntimeError::new(format!("identity '{identity}' is not dormant"))
            })?;
        let payload = self.provider.load_backing(handle.token)?.ok_or_else(|| {
            RuntimeError::new(format!("missing dormant backing for '{identity}'"))
        })?;
        let values = decode_backing(&self.checked, identity, &handle, &payload)?;

        if self.runtime.dynamic_model_types.get(identity) != Some(&handle.model_name) {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' changed model type"
            )));
        }
        if self.runtime.dynamic_model_owners.get(identity) != Some(&handle.owner) {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' changed lifetime owner"
            )));
        }

        let template = self
            .checked
            .runtime_model_templates
            .get(&handle.model_name)
            .cloned()
            .expect("backing model was validated at startup");
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
            if self.runtime.states.contains_key(&name) || self.runtime.derived.contains_key(&name) {
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
                            .expect("validated backing should contain every stored member"),
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
            self.runtime.states.insert(name, state);
        }
        for (name, cell) in derived {
            self.runtime.derived.insert(name, cell);
        }
        self.dormant.remove(identity);
        Ok(())
    }

    fn capture_complete_image(&mut self) -> Result<PersistenceImage, RuntimeError> {
        let mut image = capture_image(&self.runtime, &self.shape)?;
        let dormant = self.dormant.clone();
        for (identity, handle) in dormant {
            let payload = self.provider.load_backing(handle.token)?.ok_or_else(|| {
                RuntimeError::new(format!("missing dormant backing for '{identity}'"))
            })?;
            let values = decode_backing(&self.checked, &identity, &handle, &payload)?;
            for (member_name, value) in values {
                let name = model_binding_name(&identity, &member_name);
                if image.state_values.insert(name, value).is_some() {
                    return Err(RuntimeError::new(format!(
                        "complete capture duplicated dormant member '{identity}.{member_name}'"
                    )));
                }
            }
        }
        Ok(image)
    }

    fn into_provider(self) -> P {
        self.provider
    }
}

fn seeded_world() -> (crate::CheckedSource, PersistenceImage, HashSet<String>) {
    let checked = checked_source();
    let shape = persistence_shape(&checked).expect("persistence shape should build");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    runtime.run_action("seed").expect("seed should commit");
    let image = capture_image(&runtime, &shape).expect("seeded world should capture");
    let cold = cold_subtree_identities(&image);
    assert_eq!(cold.len(), 2, "Cold Folder + Cold Document");
    (checked, image, cold)
}

#[test]
fn opaque_host_backing_loads_only_the_explicitly_materialized_identity() {
    let (checked, image, cold) = seeded_world();
    let original_dynamic = image.dynamic_models.clone();
    let allocator = image.next_dynamic_identity;
    let provider = split_world_into_opaque_backing(&checked, &image, &cold)
        .expect("runtime-owned split should succeed");
    let mut app = OpaqueBackedRuntime::open(checked, provider)
        .expect("runtime should open from opaque manifest without Cold backing loads");

    assert_eq!(app.provider.manifest_loads, 1);
    assert!(app.provider.backing_loads.is_empty());
    assert_eq!(
        app.runtime.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
    assert_eq!(
        app.runtime.value("coldInWorkspace").unwrap(),
        Value::Bool(true)
    );
    app.runtime.run_action("renameActive").unwrap();
    assert!(
        app.provider.backing_loads.is_empty(),
        "ordinary Active work must not fetch Cold backing"
    );
    app.runtime
        .value("coldName")
        .expect_err("ordinary dormant access must not auto-materialize");
    assert!(app.provider.backing_loads.is_empty());

    let cold_folder = cold
        .iter()
        .find(|identity| original_dynamic[*identity].model_name == "Folder")
        .cloned()
        .unwrap();
    let cold_document = cold
        .iter()
        .find(|identity| original_dynamic[*identity].model_name == "Document")
        .cloned()
        .unwrap();
    let owners_before = app.runtime.dynamic_model_owners.clone();
    let types_before = app.runtime.dynamic_model_types.clone();

    app.materialize(&cold_folder)
        .expect("explicit Folder materialization should fetch exactly its opaque payload");
    assert_eq!(app.provider.backing_loads.len(), 1);
    assert_eq!(
        app.runtime.value("coldName").unwrap(),
        Value::String("Cold".to_string())
    );
    assert_eq!(
        app.runtime.value("coldLabel").unwrap(),
        Value::String("Cold folder".to_string())
    );
    app.runtime
        .value("coldDocumentTitle")
        .expect_err("Document should remain independently dormant");
    assert_eq!(app.provider.backing_loads.len(), 1);

    app.materialize(&cold_document)
        .expect("explicit Document materialization should fetch its own opaque payload");
    assert_eq!(app.provider.backing_loads.len(), 2);
    assert_eq!(
        app.runtime.value("coldDocumentTitle").unwrap(),
        Value::String("Cold document".to_string())
    );
    assert_eq!(app.runtime.dynamic_model_owners, owners_before);
    assert_eq!(app.runtime.dynamic_model_types, types_before);
    assert_eq!(app.runtime.next_dynamic_identity, allocator);
}

#[test]
fn corrupt_or_missing_backing_fails_before_installing_any_member_cells() {
    let (checked, image, cold) = seeded_world();
    let provider = split_world_into_opaque_backing(&checked, &image, &cold).unwrap();
    let mut app = OpaqueBackedRuntime::open(checked, provider).unwrap();
    let identity = cold.iter().next().cloned().unwrap();
    let handle = app.dormant[&identity].clone();
    let template = app.checked.runtime_model_templates[&handle.model_name].clone();
    let member_names = template
        .members
        .iter()
        .map(|member| model_binding_name(&identity, &member.name))
        .collect::<Vec<_>>();

    let original = app.provider.backing.remove(&handle.token).unwrap();
    let missing = app
        .materialize(&identity)
        .expect_err("missing backing must fail");
    assert!(missing.message.contains("missing dormant backing"));
    assert!(member_names.iter().all(|name| {
        !app.runtime.states.contains_key(name) && !app.runtime.derived.contains_key(name)
    }));
    assert!(app.dormant.contains_key(&identity));

    app.provider
        .backing
        .insert(handle.token, original[..original.len() - 1].to_vec());
    let corrupt = app
        .materialize(&identity)
        .expect_err("truncated backing must fail");
    assert!(corrupt.message.contains("truncated"));
    assert!(member_names.iter().all(|name| {
        !app.runtime.states.contains_key(name) && !app.runtime.derived.contains_key(name)
    }));
    assert!(app.dormant.contains_key(&identity));
}

#[test]
fn process_restart_can_open_dormant_world_without_loading_member_backing() {
    let (checked, image, cold) = seeded_world();
    let provider = split_world_into_opaque_backing(&checked, &image, &cold).unwrap();
    let first = OpaqueBackedRuntime::open(checked.clone(), provider).unwrap();
    assert!(first.provider.backing_loads.is_empty());
    let mut provider = first.into_provider();
    provider.backing_loads.clear();

    let mut restarted = OpaqueBackedRuntime::open(checked, provider).unwrap();
    assert!(restarted.provider.backing_loads.is_empty());
    assert_eq!(
        restarted.runtime.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
    assert_eq!(
        restarted.runtime.value("coldInWorkspace").unwrap(),
        Value::Bool(true)
    );
    restarted
        .runtime
        .value("coldName")
        .expect_err("Cold should still be dormant after process-equivalent restart");
    assert!(restarted.provider.backing_loads.is_empty());
}

#[test]
fn current_complete_persistence_capture_still_fetches_every_dormant_payload() {
    let (checked, image, cold) = seeded_world();
    let provider = split_world_into_opaque_backing(&checked, &image, &cold).unwrap();
    let mut app = OpaqueBackedRuntime::open(checked.clone(), provider).unwrap();

    app.runtime.run_action("renameActive").unwrap();
    assert!(
        app.provider.backing_loads.is_empty(),
        "executing an Active-only action should not need Cold payloads"
    );

    let complete = app
        .capture_complete_image()
        .expect("existing complete-image persistence can still be reconstructed");
    assert_eq!(
        app.provider.backing_loads.len(),
        cold.len(),
        "building one complete PersistenceImage still requires every dormant payload"
    );

    let shape = persistence_shape(&checked).unwrap();
    let mut expected = Runtime::from_checked_source(&checked).unwrap();
    expected.run_action("seed").unwrap();
    expected.run_action("renameActive").unwrap();
    assert_eq!(complete, capture_image(&expected, &shape).unwrap());
}
