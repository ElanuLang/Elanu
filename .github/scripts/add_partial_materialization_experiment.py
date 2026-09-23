from pathlib import Path

persistence = Path("compiler/src/runtime/persistence.rs")
text = persistence.read_text()
anchor = "#[cfg(test)]\nmod tests {\n"
addition = "#[cfg(test)]\nmod partial_materialization_experiment;\n\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected persistence tests anchor once, found {text.count(anchor)}")
if addition not in text:
    text = text.replace(anchor, addition + anchor, 1)
persistence.write_text(text)

path = Path("compiler/src/runtime/persistence/partial_materialization_experiment.rs")
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(r'''use super::*;
use crate::check_source_with_runtime_models;
use std::collections::HashSet;

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
derived coldDocumentTitle = coldFolder.documents[0].title

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
"#;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE)
        .expect("partial-materialization pressure source should check")
}

fn cold_subtree_identities(
    runtime: &Runtime,
    image: &PersistenceImage,
) -> HashSet<String> {
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

    let mut fresh = Runtime::from_checked_source(&checked).expect("fresh runtime should initialize");
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
    let mut fresh = Runtime::from_checked_source(&checked).expect("fresh runtime should initialize");
    restore_image(&mut fresh, &shape, &omitted_cold)
        .expect("current restore does not validate every designation target eagerly");

    assert_eq!(
        fresh.value("activeName").unwrap(),
        Value::String("Active".to_string())
    );
    let error = fresh
        .value("coldName")
        .expect_err("a designation to an omitted identity cannot provide owner-relative member state");
    assert!(
        error.message.contains("unknown") || error.message.contains("missing"),
        "unexpected omitted-identity failure: {}",
        error.message
    );
}
''')
