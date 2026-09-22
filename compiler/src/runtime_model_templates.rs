use std::collections::HashMap;

use crate::ast::{Declaration, Expr, Program, StateModelDecl, StateModelMember};
use crate::diagnostic::Diagnostic;
use crate::model_types;
use crate::semantic::ValueType;

/// Whether a runtime-instantiable state-model member owns stored mutable state
/// or is computed from the model's other members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeModelMemberKind {
    State,
    Derived,
}

/// One checked member of a state-model template.
///
/// Expressions intentionally remain in model-local form here. A later runtime
/// instantiation step may root them at a concrete live identity without
/// reparsing source text or recovering member meaning from generated binding
/// names.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeModelMemberTemplate {
    pub name: String,
    pub kind: RuntimeModelMemberKind,
    pub value_type: ValueType,
    pub expression: Expr,
}

/// Structured compiler metadata sufficient to describe one state-model shape
/// before it is instantiated as a concrete live identity.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeModelTemplate {
    pub name: String,
    pub members: Vec<RuntimeModelMemberTemplate>,
}

impl RuntimeModelTemplate {
    pub fn member(&self, name: &str) -> Option<&RuntimeModelMemberTemplate> {
        self.members.iter().find(|member| member.name == name)
    }
}

/// One statically declared modeled-state root preserved for runtime ownership
/// decisions after bootstrap lowering has flattened its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModelRoot {
    pub name: String,
    pub model_name: String,
}

/// Resolve state-model declarations into typed runtime templates.
///
/// This is intentionally a narrow analysis layer, not a second semantic
/// checker or a general IR. It reuses the shared state-model `ValueType`
/// analysis and preserves already-parsed member expressions so later dynamic
/// instantiation does not need to reconstruct semantic structure from the
/// flattened bootstrap representation.
pub fn collect(
    declarations: &[StateModelDecl],
) -> Result<HashMap<String, RuntimeModelTemplate>, Vec<Diagnostic>> {
    let type_facts = model_types::resolve(declarations)?;
    let mut templates = HashMap::new();

    for model in declarations {
        let resolved = type_facts
            .model(&model.name)
            .expect("validated state model should have shared type facts");

        let members = model
            .members
            .iter()
            .map(|member| {
                let name = member.name().to_string();
                let value_type = resolved
                    .member_type(&name)
                    .expect("validated state-model member should have a shared value type")
                    .clone();

                let (kind, mut expression) = match member {
                    StateModelMember::State(state) => {
                        (RuntimeModelMemberKind::State, state.initializer.clone())
                    }
                    StateModelMember::Derived(derived) => {
                        (RuntimeModelMemberKind::Derived, derived.expression.clone())
                    }
                };

                if let Expr::Filter {
                    source,
                    element_model,
                    ..
                } = &mut expression
                {
                    if element_model.is_none() {
                        if let Expr::Name(source_name) = source.as_ref() {
                            if let Some(ValueType::SequenceLive(model)) =
                                resolved.member_type(source_name)
                            {
                                *element_model = Some(model.clone());
                            }
                        }
                    }
                }

                RuntimeModelMemberTemplate {
                    name,
                    kind,
                    value_type,
                    expression,
                }
            })
            .collect();

        templates.insert(
            model.name.clone(),
            RuntimeModelTemplate {
                name: model.name.clone(),
                members,
            },
        );
    }

    Ok(templates)
}

/// Preserve the source-level relationship between one modeled-state root and
/// its model before model lowering replaces that root with generated member
/// bindings.
///
/// This table is metadata, not a second semantic checker. Invalid roots are
/// still diagnosed by the existing model-lowering path; only roots whose model
/// already has a runtime template are retained here.
pub fn collect_roots(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
) -> HashMap<String, RuntimeModelRoot> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            if !state.implicit_model_initializer {
                return None;
            }
            let model_name = state.type_name.as_ref()?;
            templates.contains_key(model_name).then(|| {
                (
                    state.name.clone(),
                    RuntimeModelRoot {
                        name: state.name.clone(),
                        model_name: model_name.clone(),
                    },
                )
            })
        })
        .collect()
}
