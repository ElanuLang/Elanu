#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UniqueOccurrenceError {
    NoCurrentOccurrence,
    AmbiguousCurrentOccurrence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelativeDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelativeNeighborError {
    NoCurrentOccurrence,
    AmbiguousCurrentOccurrence,
    Boundary,
}

/// Resolve one child identity to exactly one occurrence in the current ordered
/// runtime structure.
///
/// The returned index is a transient implementation location only. It is not a
/// source-visible position and must not be persisted as child or occurrence
/// identity. Callers consume it immediately while operating on the same current
/// sequence/view value.
pub(crate) fn resolve_unique_occurrence(
    designation: &str,
    targets: &[String],
) -> Result<usize, UniqueOccurrenceError> {
    let mut matching_index = None;

    for (index, target) in targets.iter().enumerate() {
        if target != designation {
            continue;
        }
        if matching_index.replace(index).is_some() {
            return Err(UniqueOccurrenceError::AmbiguousCurrentOccurrence);
        }
    }

    matching_index.ok_or(UniqueOccurrenceError::NoCurrentOccurrence)
}

/// Resolve the exact neighboring child identity in the current ordered structure.
///
/// The occurrence index remains transient implementation state. This helper
/// consumes it immediately against the same current ordered structure and
/// returns only the neighboring child identity.
pub(crate) fn resolve_unique_neighbor(
    designation: &str,
    targets: &[String],
    direction: RelativeDirection,
) -> Result<String, RelativeNeighborError> {
    let index = match resolve_unique_occurrence(designation, targets) {
        Ok(index) => index,
        Err(UniqueOccurrenceError::NoCurrentOccurrence) => {
            return Err(RelativeNeighborError::NoCurrentOccurrence)
        }
        Err(UniqueOccurrenceError::AmbiguousCurrentOccurrence) => {
            return Err(RelativeNeighborError::AmbiguousCurrentOccurrence)
        }
    };

    let neighbor = match direction {
        RelativeDirection::Previous => index.checked_sub(1),
        RelativeDirection::Next => index.checked_add(1).filter(|next| *next < targets.len()),
    }
    .ok_or(RelativeNeighborError::Boundary)?;

    Ok(targets[neighbor].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn unique_occurrence_resolves_only_current_transient_location() {
        assert_eq!(
            resolve_unique_occurrence("B", &targets(&["A", "B", "C"])),
            Ok(1)
        );
        assert_eq!(
            resolve_unique_occurrence("B", &targets(&["X", "A", "B", "C"])),
            Ok(2)
        );
    }

    #[test]
    fn zero_and_multiple_current_occurrences_are_distinct_failures() {
        assert_eq!(
            resolve_unique_occurrence("D", &targets(&["A", "B", "C"])),
            Err(UniqueOccurrenceError::NoCurrentOccurrence)
        );
        assert_eq!(
            resolve_unique_occurrence("C", &targets(&["A", "C", "B", "C", "D"])),
            Err(UniqueOccurrenceError::AmbiguousCurrentOccurrence)
        );
    }

    #[test]
    fn duplicates_of_other_children_do_not_make_unique_anchor_ambiguous() {
        assert_eq!(
            resolve_unique_occurrence("B", &targets(&["A", "B", "C", "C"])),
            Ok(1)
        );
    }

    #[test]
    fn neighbor_resolution_returns_identity_not_occurrence_position() {
        let structure = targets(&["A", "B", "C"]);

        assert_eq!(
            resolve_unique_neighbor("B", &structure, RelativeDirection::Previous),
            Ok("A".to_string())
        );
        assert_eq!(
            resolve_unique_neighbor("B", &structure, RelativeDirection::Next),
            Ok("C".to_string())
        );
    }

    #[test]
    fn neighbor_resolution_preserves_occurrence_failures_and_boundary() {
        assert_eq!(
            resolve_unique_neighbor("D", &targets(&["A", "B", "C"]), RelativeDirection::Next,),
            Err(RelativeNeighborError::NoCurrentOccurrence)
        );
        assert_eq!(
            resolve_unique_neighbor(
                "C",
                &targets(&["A", "C", "B", "C", "D"]),
                RelativeDirection::Next,
            ),
            Err(RelativeNeighborError::AmbiguousCurrentOccurrence)
        );
        assert_eq!(
            resolve_unique_neighbor("A", &targets(&["A", "B", "C"]), RelativeDirection::Previous,),
            Err(RelativeNeighborError::Boundary)
        );
        assert_eq!(
            resolve_unique_neighbor("C", &targets(&["A", "B", "C"]), RelativeDirection::Next,),
            Err(RelativeNeighborError::Boundary)
        );
    }
}
