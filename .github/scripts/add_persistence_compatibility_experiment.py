from pathlib import Path

path = Path("compiler/src/runtime/restart_checkpoint_experiment.rs")
text = path.read_text()
anchor = "\n#[test]\nfn checkpoint_rejects_active_transaction_and_does_not_capture_staged_work() {\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected one insertion anchor, found {text.count(anchor)}")

addition = r'''
#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistenceShape {
    static_states: Vec<(String, String, Option<(String, bool)>)>,
    model_states: Vec<(String, String, String, Option<(String, bool)>)>,
    roots: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
struct CompatibleRuntimeImage {
    shape: PersistenceShape,
    checkpoint: RuntimeCheckpoint,
}

fn designation_shape(metadata: &Option<crate::designation_runtime_metadata::RuntimeDesignationMetadata>) -> Option<(String, bool)> {
    metadata
        .as_ref()
        .map(|metadata| (metadata.model_name.clone(), metadata.allows_none))
}

fn persistence_shape(checked: &crate::CheckedSource) -> Result<PersistenceShape, RuntimeError> {
    let runtime = Runtime::from_checked_source(checked)?;

    let mut static_states = runtime
        .states
        .iter()
        .map(|(name, state)| {
            (
                name.clone(),
                format!("{:?}", state.value_type),
                designation_shape(&state.designation),
            )
        })
        .collect::<Vec<_>>();
    static_states.sort();

    let mut model_states = checked
        .runtime_model_templates
        .values()
        .flat_map(|template| {
            template
                .members
                .iter()
                .filter(|member| member.kind == RuntimeModelMemberKind::State)
                .map(|member| {
                    (
                        template.name.clone(),
                        member.name.clone(),
                        format!("{:?}", member.value_type),
                        designation_shape(&member.designation),
                    )
                })
        })
        .collect::<Vec<_>>();
    model_states.sort();

    let mut roots = checked
        .runtime_model_roots
        .values()
        .map(|root| (root.name.clone(), root.model_name.clone()))
        .collect::<Vec<_>>();
    roots.sort();

    Ok(PersistenceShape {
        static_states,
        model_states,
        roots,
    })
}

fn capture_compatible_image(
    runtime: &Runtime,
    checked: &crate::CheckedSource,
) -> Result<CompatibleRuntimeImage, RuntimeError> {
    Ok(CompatibleRuntimeImage {
        shape: persistence_shape(checked)?,
        checkpoint: runtime.capture_restart_checkpoint()?,
    })
}

fn restore_compatible_image(
    runtime: &mut Runtime,
    checked: &crate::CheckedSource,
    image: &CompatibleRuntimeImage,
) -> Result<(), RuntimeError> {
    let current_shape = persistence_shape(checked)?;
    if current_shape != image.shape {
        return Err(RuntimeError::new(
            "persistence image is incompatible with the current checked application shape",
        ));
    }
    runtime.restore_restart_checkpoint(&image.checkpoint)
}

#[test]
fn identical_checked_shape_accepts_the_stored_image() {
    let checked = checked_source();
    let mut original = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut original);
    let image = capture_compatible_image(&original, &checked).expect("image should capture");

    let same_checked = checked_source();
    let mut restarted = Runtime::from_checked_source(&same_checked).expect("runtime should initialize");
    restore_compatible_image(&mut restarted, &same_checked, &image)
        .expect("identical checked shape should accept image");

    assert_eq!(
        restarted.capture_restart_checkpoint().unwrap(),
        image.checkpoint
    );
}

#[test]
fn incompatible_stored_state_shape_is_rejected_before_restore_mutation() {
    let checked = checked_source();
    let mut original = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut original);
    let image = capture_compatible_image(&original, &checked).expect("image should capture");

    let incompatible_source = SOURCE.replace(
        "state restoreParent: maybe live Folder = none",
        "state restoreParent: maybe live Folder = none\n    state archived = false",
    );
    let incompatible_checked = check_source_with_runtime_models(&incompatible_source)
        .expect("incompatible pressure source should still check");
    let mut restarted = Runtime::from_checked_source(&incompatible_checked)
        .expect("fresh incompatible runtime should initialize");
    let before = restarted
        .capture_restart_checkpoint()
        .expect("fresh runtime should checkpoint");

    let error = restore_compatible_image(&mut restarted, &incompatible_checked, &image)
        .expect_err("material state-shape mismatch must be rejected");
    assert!(error.message.contains("incompatible"));
    assert_eq!(
        restarted.capture_restart_checkpoint().unwrap(),
        before,
        "compatibility rejection must occur before restore mutation",
    );
}

#[test]
fn formatting_and_action_body_changes_do_not_change_persistence_shape() {
    let checked = checked_source();
    let formatted_source = format!("\n\n{}\n\n", SOURCE.replace("\n", "\n    "));
    let formatted = check_source_with_runtime_models(&formatted_source)
        .expect("formatting-only source should still check");
    assert_eq!(
        persistence_shape(&checked).unwrap(),
        persistence_shape(&formatted).unwrap(),
        "source formatting must not define persistence compatibility",
    );

    let changed_action_source = SOURCE.replace(
        "through another.name = \"Another\"",
        "through another.name = \"Another after upgrade\"",
    );
    let changed_action = check_source_with_runtime_models(&changed_action_source)
        .expect("action-body-only source should still check");
    assert_eq!(
        persistence_shape(&checked).unwrap(),
        persistence_shape(&changed_action).unwrap(),
        "action behavior may change without changing persisted reconstruction shape",
    );
}
'''

path.write_text(text.replace(anchor, "\n" + addition + anchor, 1))
