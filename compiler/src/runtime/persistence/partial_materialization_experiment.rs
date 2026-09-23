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
derived coldDocumentLabel = coldFolder.documents[0].label
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

action mutateActiveThenCold {
    through activeFolder.name = "Should roll back"
    through coldFolder.name = "Changed"
}
"#;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE)
        .expect("partial-materialization pressure source should check")
}

fn cold_subtree_identities(runtime: &Runtime, image: &PersistenceImage) -> HashSet<String> {
    let cold_folder = image
        .dynamic_models
        .iter()
        .find_map(|(identity, dynamic)| {
            if dynamic.model_name != "Folder" {
                return None;
            }
            let name = model_binding_name(identity, "name");
            match image.state_values.get(&name) {
                Some(Value::String(value)) if value == "Cold" => Some(identity.clone()),
                _ => None,
            }
        })
        .expect("Cold folder identity should exist in the committed image");

    assert!(runtime
        .runtime_model_templates
        .contains_key(&image.dynamic_models[&cold_folder].model_name));

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

fn remove_member_state_for_identities(
    checked: &crate::CheckedSource,
    image: &mut PersistenceImage,
    identities: &HashSet<String>,
) {
    for identity in identities {
        let dynamic = image
            .dynamic_models
            .get(identity)
            .expect("subtree identity should retain structured dynamic metadata");
        let template = checked
            .runtime_model_templates
            .get(&dynamic.model_name)
            .expect("dynamic model type should have a checked template");
        for member in template
            .members
            .iter()
            .filter(|member| member.kind == RuntimeModelMemberKind::State)
        {
            image
                .state_values
                .remove(&model_binding_name(identity, &member.name));
        }
    }
}

#[derive(Debug, Clone)]
struct DormantIdentity {
    model_name: String,
    owner: String,
    state_values: HashMap<String, Value>,
}

struct PartiallyMaterializedRuntime {
    checked: crate::CheckedSource,
    shape: PersistenceShape,
    runtime: Runtime,
    dormant: HashMap<String, DormantIdentity>,
}

impl PartiallyMaterializedRuntime {
    fn restore(
        checked: crate::CheckedSource,
        image: &PersistenceImage,
        dormant_identities: &HashSet<String>,
    ) -> Result<Self, RuntimeError> {
        let shape = persistence_shape(&checked)?;
        let mut partial = image.clone();
        let mut dormant = HashMap::new();

        for identity in dormant_identities {
            let dynamic = image.dynamic_models.get(identity).ok_or_else(|| {
                RuntimeError::new(format!("unknown dormant identity '{identity}'"))
            })?;
            let template = checked
                .runtime_model_templates
                .get(&dynamic.model_name)
                .ok_or_else(|| {
                    RuntimeError::new(format!(
                        "unknown model '{}' for dormant identity '{identity}'",
                        dynamic.model_name
                    ))
                })?;

            let mut state_values = HashMap::new();
            for member in template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
            {
                let stored_name = model_binding_name(identity, &member.name);
                let value = image
                    .state_values
                    .get(&stored_name)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeError::new(format!(
                            "persistence image is missing dormant state '{identity}.{}'",
                            member.name
                        ))
                    })?;
                state_values.insert(member.name.clone(), value);
                partial.state_values.remove(&stored_name);
            }

            dormant.insert(
                identity.clone(),
                DormantIdentity {
                    model_name: dynamic.model_name.clone(),
                    owner: dynamic.owner.clone(),
                    state_values,
                },
            );
            partial.dynamic_models.remove(identity);
        }

        let mut runtime = Runtime::from_checked_source(&checked)?;
        restore_image(&mut runtime, &shape, &partial)?;

        // Re-register exact committed identity/type/provenance without instantiating
        // model-local StateCell / DerivedCell topology.
        for (identity, dormant_identity) in &dormant {
            runtime
                .dynamic_model_types
                .insert(identity.clone(), dormant_identity.model_name.clone());
            runtime
                .dynamic_model_owners
                .insert(identity.clone(), dormant_identity.owner.clone());
        }

        Ok(Self {
            checked,
            shape,
            runtime,
            dormant,
        })
    }

    fn materialize(&mut self, identity: &str) -> Result<(), RuntimeError> {
        let dormant =
            self.dormant.get(identity).cloned().ok_or_else(|| {
                RuntimeError::new(format!("identity '{identity}' is not dormant"))
            })?;

        let current_model = self
            .runtime
            .dynamic_model_types
            .get(identity)
            .ok_or_else(|| RuntimeError::new(format!("unknown modeled identity '{identity}'")))?;
        if current_model != &dormant.model_name {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' changed model type"
            )));
        }
        let current_owner = self
            .runtime
            .dynamic_model_owners
            .get(identity)
            .ok_or_else(|| RuntimeError::new(format!("identity '{identity}' lost provenance")))?;
        if current_owner != &dormant.owner {
            return Err(RuntimeError::new(format!(
                "dormant identity '{identity}' changed lifetime owner"
            )));
        }

        let template = self
            .checked
            .runtime_model_templates
            .get(&dormant.model_name)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown model '{}'", dormant.model_name)))?;
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
                RuntimeModelMemberKind::State => {
                    let value =
                        dormant
                            .state_values
                            .get(&member.name)
                            .cloned()
                            .ok_or_else(|| {
                                RuntimeError::new(format!(
                                    "dormant backing is missing state '{identity}.{}'",
                                    member.name
                                ))
                            })?;
                    states.push((
                        name,
                        StateCell {
                            value: coerce_value(value, &member.value_type)?,
                            value_type: member.value_type.clone(),
                            designation: member.designation.clone(),
                            dependents: HashSet::new(),
                        },
                    ));
                }
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

    fn capture_full_image(&self) -> Result<PersistenceImage, RuntimeError> {
        let mut image = capture_image(&self.runtime, &self.shape)?;
        for (identity, dormant) in &self.dormant {
            let dynamic = image.dynamic_models.get(identity).ok_or_else(|| {
                RuntimeError::new(format!("captured image lost dormant identity '{identity}'"))
            })?;
            if dynamic.model_name != dormant.model_name || dynamic.owner != dormant.owner {
                return Err(RuntimeError::new(format!(
                    "captured dormant metadata changed for '{identity}'"
                )));
            }
            for (member_name, value) in &dormant.state_values {
                let stored_name = model_binding_name(identity, member_name);
                if image
                    .state_values
                    .insert(stored_name, value.clone())
                    .is_some()
                {
                    return Err(RuntimeError::new(format!(
                        "captured image already contains dormant member '{identity}.{member_name}'"
                    )));
                }
            }
        }
        Ok(image)
    }
}

#[test]
fn committed_identity_metadata_cannot_currently_exist_without_resident_member_state() {
    let checked = checked_source();
    let shape = persistence_shape(&checked).expect("persistence shape should build");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    runtime.run_action("seed").expect("seed should commit");

    let full = capture_image(&runtime, &shape).expect("full committed world should capture");
    let cold_subtree = cold_subtree_identities(&runtime, &full);
    assert_eq!(cold_subtree.len(), 2, "Cold Folder + its Document");

    let mut identity_only_cold = full.clone();
    remove_member_state_for_identities(&checked, &mut identity_only_cold, &cold_subtree);

    for identity in &cold_subtree {
        assert!(identity_only_cold.dynamic_models.contains_key(identity));
    }

    let mut fresh =
        Runtime::from_checked_source(&checked).expect("fresh runtime should initialize");
    let error = restore_image(&mut fresh, &shape, &identity_only_cold)
        .expect_err("restore currently requires member state for every committed dynamic identity");
    assert!(
        error.message.contains("missing dynamic state"),
        "unexpected partial materialization failure: {}",
        error.message
    );
}

#[test]
fn dropping_nonresident_identity_metadata_is_not_a_valid_partial_world() {
    let checked = checked_source();
    let shape = persistence_shape(&checked).expect("persistence shape should build");
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    runtime.run_action("seed").expect("seed should commit");

    let full = capture_image(&runtime, &shape).expect("full committed world should capture");
    let cold_subtree = cold_subtree_identities(&runtime, &full);

    let mut omitted_cold = full.clone();
    remove_member_state_for_identities(&checked, &mut omitted_cold, &cold_subtree);
    for identity in &cold_subtree {
        omitted_cold.dynamic_models.remove(identity);
    }

    // Static workspace membership and the persistent coldFolder designation still
    // contain the exact Cold identity. Removing the dynamic identity metadata is
    // therefore not equivalent to making that identity merely nonresident.
    let mut fresh =
        Runtime::from_checked_source(&checked).expect("fresh runtime should initialize");
    restore_image(&mut fresh, &shape, &omitted_cold)
        .expect("current restore does not validate every designation target eagerly");

    assert_eq!(
        fresh.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
    let error = fresh.value("coldName").expect_err(
        "a designation to an omitted identity cannot provide owner-relative member state",
    );
    assert!(
        error.message.contains("unknown") || error.message.contains("missing"),
        "unexpected omitted-identity failure: {}",
        error.message
    );
}

#[test]
fn dormant_identity_representation_preserves_exact_world_and_materializes_in_place() {
    let checked = checked_source();
    let shape = persistence_shape(&checked).expect("persistence shape should build");
    let mut fully_materialized =
        Runtime::from_checked_source(&checked).expect("runtime should initialize");
    fully_materialized
        .run_action("seed")
        .expect("seed should commit");

    let seed_image =
        capture_image(&fully_materialized, &shape).expect("seeded world should capture");
    let cold_subtree = cold_subtree_identities(&fully_materialized, &seed_image);
    assert_eq!(cold_subtree.len(), 2, "Cold Folder + its Document");
    let cold_folder = cold_subtree
        .iter()
        .find(|identity| seed_image.dynamic_models[*identity].model_name == "Folder")
        .cloned()
        .expect("Cold Folder identity should be known");
    let cold_document = cold_subtree
        .iter()
        .find(|identity| seed_image.dynamic_models[*identity].model_name == "Document")
        .cloned()
        .expect("Cold Document identity should be known");
    let original_owners = fully_materialized.dynamic_model_owners.clone();
    let original_types = fully_materialized.dynamic_model_types.clone();
    let allocator = fully_materialized.next_dynamic_identity;

    let mut partial =
        PartiallyMaterializedRuntime::restore(checked.clone(), &seed_image, &cold_subtree)
            .expect("runtime should restore with Cold subtree dormant");

    assert_eq!(partial.runtime.dynamic_model_owners, original_owners);
    assert_eq!(partial.runtime.dynamic_model_types, original_types);
    assert_eq!(partial.runtime.next_dynamic_identity, allocator);
    assert_eq!(
        partial.runtime.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
    assert_eq!(
        partial.runtime.value("coldInWorkspace").unwrap(),
        Value::Bool(true)
    );

    let cold_read_error = partial
        .runtime
        .value("coldName")
        .expect_err("dormant member access must fail rather than fabricate defaults");
    assert!(
        cold_read_error.message.contains("unknown") || cold_read_error.message.contains("missing"),
        "unexpected dormant read failure: {}",
        cold_read_error.message
    );

    partial
        .runtime
        .run_action("mutateActiveThenCold")
        .expect_err("dormant mutation must fail transactionally");
    assert_eq!(
        partial.runtime.value("activeName").unwrap(),
        Value::String("Active".to_string()),
        "the staged Active write before the dormant failure must roll back"
    );

    // Residency is not committed application semantics: reconstructing the full
    // image from resident cells + structured dormant backing yields the exact world.
    assert_eq!(
        partial.capture_full_image().unwrap(),
        seed_image,
        "making Cold dormant must not change the committed semantic world"
    );
    assert_eq!(
        partial.capture_full_image().unwrap().encode().unwrap(),
        seed_image.encode().unwrap(),
        "dormancy must not change the opaque durable bytes"
    );

    // Ordinary resident mutation should behave identically to a fully materialized runtime.
    partial.runtime.run_action("renameActive").unwrap();
    fully_materialized.run_action("renameActive").unwrap();
    let expected_after_active = capture_image(&fully_materialized, &shape).unwrap();
    assert_eq!(partial.capture_full_image().unwrap(), expected_after_active);

    partial
        .materialize(&cold_folder)
        .expect("Cold Folder should materialize from structured backing");
    assert_eq!(partial.runtime.dynamic_model_owners, original_owners);
    assert_eq!(partial.runtime.dynamic_model_types, original_types);
    assert_eq!(partial.runtime.next_dynamic_identity, allocator);
    assert_eq!(
        partial.runtime.value("coldName").unwrap(),
        Value::String("Cold".to_string())
    );
    assert_eq!(
        partial.runtime.value("coldLabel").unwrap(),
        Value::String("Cold folder".to_string()),
        "derived topology should be reconstructed with the Folder"
    );
    partial
        .runtime
        .value("coldDocumentTitle")
        .expect_err("Cold Document should remain independently dormant");

    partial
        .materialize(&cold_document)
        .expect("Cold Document should materialize with the same identity");
    assert!(partial.dormant.is_empty());
    assert_eq!(
        partial.runtime.value("coldDocumentTitle").unwrap(),
        Value::String("Cold document".to_string())
    );
    assert_eq!(
        partial.runtime.value("coldDocumentLabel").unwrap(),
        Value::String("Cold document document".to_string()),
        "derived Document topology should be reconstructed"
    );
    assert_eq!(
        capture_image(&partial.runtime, &partial.shape).unwrap(),
        expected_after_active,
        "materialization itself must not alter the committed world"
    );

    let bytes = partial.capture_full_image().unwrap().encode().unwrap();
    let decoded = PersistenceImage::decode(&bytes).expect("opaque bytes should decode");
    let mut restarted = PartiallyMaterializedRuntime::restore(checked, &decoded, &cold_subtree)
        .expect("fresh process-equivalent runtime should restore Cold dormant again");
    assert_eq!(
        restarted.runtime.value("activeName").unwrap(),
        Value::String("Active renamed".to_string())
    );
    restarted
        .runtime
        .value("coldName")
        .expect_err("restart should preserve the chosen nonresident boundary");
    restarted.materialize(&cold_folder).unwrap();
    restarted.materialize(&cold_document).unwrap();
    assert_eq!(
        restarted.capture_full_image().unwrap().encode().unwrap(),
        bytes,
        "restart + explicit materialization must preserve the exact durable world"
    );
}
