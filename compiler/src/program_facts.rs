use std::collections::HashMap;

use crate::ast::{Declaration, Program, StateModelMember};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelMemberKind {
    State,
    Derived,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelFacts {
    pub(crate) members: HashMap<String, ModelMemberKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModeledStateRoot {
    pub(crate) name: String,
    pub(crate) model_name: String,
    pub(crate) declaration_index: usize,
}

/// Read-only structural facts derived from the current source-level `Program`.
///
/// This is intentionally not a semantic checker or compiler IR. It records only
/// source topology that bootstrap passes otherwise have to rediscover. Add new
/// facts only when an existing consumer is ready to use them.
#[derive(Debug, Clone)]
pub(crate) struct ProgramFacts {
    model_names: Vec<String>,
    models: HashMap<String, ModelFacts>,
    modeled_state_roots: Vec<ModeledStateRoot>,
}

impl ProgramFacts {
    pub(crate) fn from_program(program: &Program) -> Self {
        let model_names: Vec<String> = program
            .state_models
            .iter()
            .map(|model| model.name.clone())
            .collect();

        let models: HashMap<String, ModelFacts> = program
            .state_models
            .iter()
            .map(|model| {
                let mut members = HashMap::new();

                for member in &model.members {
                    match member {
                        StateModelMember::State(state) => {
                            members.insert(state.name.clone(), ModelMemberKind::State);
                        }
                        StateModelMember::Derived(derived) => {
                            members.insert(derived.name.clone(), ModelMemberKind::Derived);
                        }
                    }
                }

                (model.name.clone(), ModelFacts { members })
            })
            .collect();

        let modeled_state_roots = program
            .declarations
            .iter()
            .enumerate()
            .filter_map(|(declaration_index, declaration)| {
                let Declaration::State(state) = declaration else {
                    return None;
                };
                if !state.implicit_model_initializer {
                    return None;
                }

                let model_name = state.type_name.as_ref()?;
                if !models.contains_key(model_name) {
                    return None;
                }

                Some(ModeledStateRoot {
                    name: state.name.clone(),
                    model_name: model_name.clone(),
                    declaration_index,
                })
            })
            .collect();

        Self {
            model_names,
            models,
            modeled_state_roots,
        }
    }

    pub(crate) fn has_model(&self, name: &str) -> bool {
        self.model_names.iter().any(|model| model == name)
    }

    pub(crate) fn models(&self) -> &HashMap<String, ModelFacts> {
        &self.models
    }

    pub(crate) fn modeled_state_roots(&self) -> &[ModeledStateRoot] {
        &self.modeled_state_roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_source;

    #[test]
    fn collects_model_members_and_known_roots_in_source_order() {
        let program = parse_source(
            r#"
state model Invoice {
    state quantity: Int = 1
    derived remaining = quantity
}

state unrelated = 0
state first: Invoice
derived marker = unrelated
state second: Invoice
state unknown: Missing
"#,
        )
        .expect("source should parse");

        let facts = ProgramFacts::from_program(&program);

        assert!(facts.has_model("Invoice"));
        assert!(!facts.has_model("Missing"));

        let invoice = facts
            .models()
            .get("Invoice")
            .expect("Invoice model should be recorded");
        assert_eq!(
            invoice.members.get("quantity"),
            Some(&ModelMemberKind::State)
        );
        assert_eq!(
            invoice.members.get("remaining"),
            Some(&ModelMemberKind::Derived)
        );

        assert_eq!(
            facts.modeled_state_roots(),
            [
                ModeledStateRoot {
                    name: "first".to_string(),
                    model_name: "Invoice".to_string(),
                    declaration_index: 1,
                },
                ModeledStateRoot {
                    name: "second".to_string(),
                    model_name: "Invoice".to_string(),
                    declaration_index: 3,
                },
            ]
        );
    }
}
