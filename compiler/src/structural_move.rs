use crate::relative_navigation::{resolve_unique_occurrence, UniqueOccurrenceError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelativePlacement {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuralMoveError {
    MovingNoCurrentOccurrence,
    MovingAmbiguousCurrentOccurrence,
    AnchorNoCurrentOccurrence,
    AnchorAmbiguousCurrentOccurrence,
    ViewDoesNotMapToBacking,
}

/// Reorder one uniquely occurring child identity relative to another uniquely occurring
/// child identity. `selection` is either the backing membership itself or an
/// identity-preserving filtered view of it. Numeric occurrence locations are consumed only
/// while computing the returned backing value; they are not durable identity.
pub(crate) fn move_unique_relative(
    backing: &[String],
    selection: &[String],
    moving: &str,
    anchor: &str,
    placement: RelativePlacement,
) -> Result<Vec<String>, StructuralMoveError> {
    let moving_selection_index =
        resolve_unique_occurrence(moving, selection).map_err(|error| match error {
            UniqueOccurrenceError::NoCurrentOccurrence => {
                StructuralMoveError::MovingNoCurrentOccurrence
            }
            UniqueOccurrenceError::AmbiguousCurrentOccurrence => {
                StructuralMoveError::MovingAmbiguousCurrentOccurrence
            }
        })?;
    let anchor_selection_index =
        resolve_unique_occurrence(anchor, selection).map_err(|error| match error {
            UniqueOccurrenceError::NoCurrentOccurrence => {
                StructuralMoveError::AnchorNoCurrentOccurrence
            }
            UniqueOccurrenceError::AmbiguousCurrentOccurrence => {
                StructuralMoveError::AnchorAmbiguousCurrentOccurrence
            }
        })?;

    let mapped = map_selection_to_backing(backing, selection)?;
    let moving_backing_index = mapped[moving_selection_index];
    let anchor_backing_index = mapped[anchor_selection_index];

    if moving_backing_index == anchor_backing_index {
        return Ok(backing.to_vec());
    }

    let mut reordered = backing.to_vec();
    let moved = reordered.remove(moving_backing_index);
    let anchor_after_removal = if moving_backing_index < anchor_backing_index {
        anchor_backing_index - 1
    } else {
        anchor_backing_index
    };
    let insertion_index = match placement {
        RelativePlacement::Before => anchor_after_removal,
        RelativePlacement::After => anchor_after_removal + 1,
    };
    reordered.insert(insertion_index, moved);
    Ok(reordered)
}

fn map_selection_to_backing(
    backing: &[String],
    selection: &[String],
) -> Result<Vec<usize>, StructuralMoveError> {
    let mut mapped = Vec::with_capacity(selection.len());
    let mut selection_index = 0usize;

    for (backing_index, backing_target) in backing.iter().enumerate() {
        if selection_index < selection.len() && backing_target == &selection[selection_index] {
            mapped.push(backing_index);
            selection_index += 1;
        }
    }

    if selection_index == selection.len() {
        Ok(mapped)
    } else {
        Err(StructuralMoveError::ViewDoesNotMapToBacking)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn direct_membership_moves_unique_child_before_and_after_anchor() {
        let backing = values(&["A", "B", "C", "D"]);

        assert_eq!(
            move_unique_relative(&backing, &backing, "B", "D", RelativePlacement::After,).unwrap(),
            values(&["A", "C", "D", "B"])
        );
        assert_eq!(
            move_unique_relative(&backing, &backing, "D", "B", RelativePlacement::Before,).unwrap(),
            values(&["A", "D", "B", "C"])
        );
    }

    #[test]
    fn movement_relative_to_itself_is_a_noop() {
        let backing = values(&["A", "B", "C"]);
        assert_eq!(
            move_unique_relative(&backing, &backing, "B", "B", RelativePlacement::After,).unwrap(),
            backing
        );
    }

    #[test]
    fn filtered_view_maps_exact_retained_occurrences_back_to_backing() {
        let backing = values(&["A", "B", "C", "D"]);
        let view = values(&["A", "C", "D"]);

        assert_eq!(
            move_unique_relative(&backing, &view, "C", "D", RelativePlacement::After,).unwrap(),
            values(&["A", "B", "D", "C"])
        );
    }

    #[test]
    fn hidden_rows_keep_relative_order_during_filtered_movement() {
        let backing = values(&["A", "hidden1", "B", "hidden2", "C", "D"]);
        let view = values(&["A", "B", "C", "D"]);

        assert_eq!(
            move_unique_relative(&backing, &view, "D", "B", RelativePlacement::Before,).unwrap(),
            values(&["A", "hidden1", "D", "B", "hidden2", "C"])
        );
    }

    #[test]
    fn moving_or_anchor_duplicates_are_ambiguous_but_unrelated_duplicates_are_not() {
        let duplicate_moving = values(&["A", "B", "B", "C"]);
        assert_eq!(
            move_unique_relative(
                &duplicate_moving,
                &duplicate_moving,
                "B",
                "C",
                RelativePlacement::After,
            ),
            Err(StructuralMoveError::MovingAmbiguousCurrentOccurrence)
        );

        let duplicate_anchor = values(&["A", "B", "C", "C"]);
        assert_eq!(
            move_unique_relative(
                &duplicate_anchor,
                &duplicate_anchor,
                "B",
                "C",
                RelativePlacement::After,
            ),
            Err(StructuralMoveError::AnchorAmbiguousCurrentOccurrence)
        );

        let unrelated_duplicate = values(&["A", "X", "X", "B", "C"]);
        assert_eq!(
            move_unique_relative(
                &unrelated_duplicate,
                &unrelated_duplicate,
                "B",
                "C",
                RelativePlacement::After,
            )
            .unwrap(),
            values(&["A", "X", "X", "C", "B"])
        );
    }

    #[test]
    fn missing_occurrences_and_invalid_view_mapping_fail_explicitly() {
        let backing = values(&["A", "B", "C"]);
        assert_eq!(
            move_unique_relative(&backing, &backing, "D", "C", RelativePlacement::After,),
            Err(StructuralMoveError::MovingNoCurrentOccurrence)
        );
        assert_eq!(
            move_unique_relative(&backing, &backing, "B", "D", RelativePlacement::After,),
            Err(StructuralMoveError::AnchorNoCurrentOccurrence)
        );

        let impossible_view = values(&["B", "A"]);
        assert_eq!(
            move_unique_relative(
                &backing,
                &impossible_view,
                "B",
                "A",
                RelativePlacement::After,
            ),
            Err(StructuralMoveError::ViewDoesNotMapToBacking)
        );
    }
}
