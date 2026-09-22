use std::collections::HashMap;

use crate::ast::{decode_live_type_name, decode_maybe_live_type_name, Declaration, Program};

pub(crate) const LOWERED_DESIGNATION_PREFIX: &str = "__meld_live$";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeDesignationMetadata {
    pub(crate) model_name: String,
    pub(crate) allows_none: bool,
}

pub(crate) fn collect(program: &Program) -> HashMap<String, RuntimeDesignationMetadata> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            let type_name = state.type_name.as_deref()?;
            let (model_name, allows_none) = if let Some(model) = decode_live_type_name(type_name) {
                (model, false)
            } else {
                let model = decode_maybe_live_type_name(type_name)?;
                (model, true)
            };

            Some((
                format!("{LOWERED_DESIGNATION_PREFIX}{}", state.name),
                RuntimeDesignationMetadata {
                    model_name: model_name.to_string(),
                    allows_none,
                },
            ))
        })
        .collect()
}
