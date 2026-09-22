use super::relative_navigation::{
    resolve_unique_neighbor as resolve_runtime_neighbor, RelativeDirection, RelativeNeighborError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolutionError {
    AbsentDesignation,
    NoCurrentOccurrence,
    AmbiguousCurrentOccurrence,
    Boundary,
}

/// Pressure-only semantic wrapper for relative navigation.
///
/// This deliberately models only the remaining source-policy distinction the
/// pressure test still needs: a designation may be absent before runtime
/// neighbor resolution begins. Unique occurrence, direction, boundary, and
/// neighboring identity are all delegated to the production runtime helper.
fn resolve_unique_neighbor(
    designation: Option<&str>,
    current_structure: &[&str],
    direction: RelativeDirection,
) -> Result<String, ResolutionError> {
    let designation = designation.ok_or(ResolutionError::AbsentDesignation)?;
    let targets: Vec<String> = current_structure
        .iter()
        .map(|target| (*target).to_string())
        .collect();

    resolve_runtime_neighbor(designation, &targets, direction).map_err(|error| match error {
        RelativeNeighborError::NoCurrentOccurrence => ResolutionError::NoCurrentOccurrence,
        RelativeNeighborError::AmbiguousCurrentOccurrence => {
            ResolutionError::AmbiguousCurrentOccurrence
        }
        RelativeNeighborError::Boundary => ResolutionError::Boundary,
    })
}

#[test]
fn unique_middle_occurrence_resolves_immediate_next_and_previous_child() {
    let structure = ["A", "B", "C"];

    assert_eq!(
        resolve_unique_neighbor(Some("B"), &structure, RelativeDirection::Next),
        Ok("C".to_string())
    );
    assert_eq!(
        resolve_unique_neighbor(Some("B"), &structure, RelativeDirection::Previous),
        Ok("A".to_string())
    );
}

#[test]
fn resolution_uses_current_structure_not_a_persisted_numeric_position() {
    let before = ["A", "B", "C"];
    let after_earlier_row_leaves = ["B", "C"];
    let after_earlier_row_enters = ["X", "A", "B", "C"];

    assert_eq!(
        resolve_unique_neighbor(Some("B"), &before, RelativeDirection::Next),
        Ok("C".to_string())
    );
    assert_eq!(
        resolve_unique_neighbor(
            Some("B"),
            &after_earlier_row_leaves,
            RelativeDirection::Next,
        ),
        Ok("C".to_string())
    );
    assert_eq!(
        resolve_unique_neighbor(
            Some("B"),
            &after_earlier_row_enters,
            RelativeDirection::Next,
        ),
        Ok("C".to_string())
    );
}

#[test]
fn filtered_view_changes_are_just_current_ordered_structure_changes() {
    let filtered_before = ["A", "B", "D"];
    let filtered_after_predicate_change = ["A", "B", "C", "D"];

    assert_eq!(
        resolve_unique_neighbor(Some("B"), &filtered_before, RelativeDirection::Next),
        Ok("D".to_string())
    );
    assert_eq!(
        resolve_unique_neighbor(
            Some("B"),
            &filtered_after_predicate_change,
            RelativeDirection::Next,
        ),
        Ok("C".to_string())
    );
}

#[test]
fn absent_designation_and_zero_current_occurrences_are_distinct_failures() {
    let structure = ["A", "B", "C"];

    assert_eq!(
        resolve_unique_neighbor(None, &structure, RelativeDirection::Next),
        Err(ResolutionError::AbsentDesignation)
    );
    assert_eq!(
        resolve_unique_neighbor(Some("D"), &structure, RelativeDirection::Next),
        Err(ResolutionError::NoCurrentOccurrence)
    );
}

#[test]
fn duplicate_current_occurrences_are_ambiguous_even_when_neighbors_differ() {
    let structure = ["A", "C", "B", "C", "D"];

    assert_eq!(
        resolve_unique_neighbor(Some("C"), &structure, RelativeDirection::Next),
        Err(ResolutionError::AmbiguousCurrentOccurrence)
    );
    assert_eq!(
        resolve_unique_neighbor(Some("C"), &structure, RelativeDirection::Previous),
        Err(ResolutionError::AmbiguousCurrentOccurrence)
    );
}

#[test]
fn duplicate_neighbor_identity_does_not_make_a_unique_anchor_ambiguous() {
    let structure = ["A", "B", "C", "C"];

    assert_eq!(
        resolve_unique_neighbor(Some("B"), &structure, RelativeDirection::Next),
        Ok("C".to_string())
    );
}

#[test]
fn boundary_is_explicit_failure_in_the_pressure_model() {
    let structure = ["A", "B", "C"];

    assert_eq!(
        resolve_unique_neighbor(Some("A"), &structure, RelativeDirection::Previous),
        Err(ResolutionError::Boundary)
    );
    assert_eq!(
        resolve_unique_neighbor(Some("C"), &structure, RelativeDirection::Next),
        Err(ResolutionError::Boundary)
    );
}

#[test]
fn empty_structure_reports_no_current_occurrence_before_boundary_policy() {
    let structure: [&str; 0] = [];

    assert_eq!(
        resolve_unique_neighbor(Some("A"), &structure, RelativeDirection::Next),
        Err(ResolutionError::NoCurrentOccurrence)
    );
}
