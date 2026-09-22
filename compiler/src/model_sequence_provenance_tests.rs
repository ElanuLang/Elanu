use crate::{model_sequence_integration, parse_source};

#[test]
fn externalized_model_sequences_preserve_owner_and_member_provenance() {
    let source = r#"
state model Task {
    state title = ""
}

state model Project {
    state tasks: [live Task] = []
}

state projectA: Project
state projectB: Project
"#;

    let program = parse_source(source).expect("source should parse");
    let lowered = model_sequence_integration::lower(&program).expect("integration should lower");

    let mut provenance: Vec<_> = lowered.externalized_sequences.values().collect();
    provenance.sort_by(|left, right| left.owner_root.cmp(&right.owner_root));

    assert_eq!(provenance.len(), 2);
    assert_eq!(provenance[0].owner_root, "projectA");
    assert_eq!(provenance[0].owner_model, "Project");
    assert_eq!(provenance[0].member_name, "tasks");
    assert_eq!(provenance[0].element_model, "Task");
    assert_eq!(provenance[1].owner_root, "projectB");
    assert_eq!(provenance[1].owner_model, "Project");
    assert_eq!(provenance[1].member_name, "tasks");
    assert_eq!(provenance[1].element_model, "Task");
}
