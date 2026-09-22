use std::collections::{HashMap, HashSet};

use crate::ast::{
    decode_live_type_name, decode_maybe_live_type_name, decode_through_path, ActionArgument,
    ActionDecl, AssignmentOperator, BinaryOperator, Declaration, DerivedDecl, Expr, Program,
    SourceLocation, StateDecl, StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::filter_integration::MODEL_FILTER_BINDING_PREFIX;
use crate::model_sequence_integration::ExternalizedModelSequence;
use crate::program_facts::{
    ModelFacts as ModelInfo, ModelMemberKind as MemberKind, ModeledStateRoot as RootInfo,
    ProgramFacts,
};
use crate::runtime_model_templates::{RuntimeModelMemberKind, RuntimeModelTemplate};
use crate::runtime_sequence_markers::encode_runtime_sequence_type;
use crate::scoped_designation_surface::decode_scope_identity_param;
use crate::semantic::ValueType;
use crate::sequence_surface::{
    decode_sequence_literal, decode_sequence_live_type, SEQUENCE_INDEX_SEGMENT_PREFIX,
};

const LOWERED_SEQUENCE_VALUE_PREFIX: &str = "__meld_sequence_value$";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceVariant {
    targets: Vec<String>,
    encoded: String,
}

#[derive(Debug, Clone)]
struct SequenceInfo {
    name: String,
    model_name: String,
    declaration_index: usize,
    candidates: HashSet<String>,
    variants: Vec<SequenceVariant>,
    externalized_model_member: bool,
}

struct LoweringContext<'a> {
    declaration_index: usize,
    sequences: &'a HashMap<String, SequenceInfo>,
    filter_views: &'a HashMap<String, String>,
    designation_models: &'a HashMap<String, (String, usize)>,
    models: &'a HashMap<String, ModelInfo>,
    runtime_model_templates: &'a HashMap<String, RuntimeModelTemplate>,
}

#[derive(Debug, Clone, Copy)]
struct IndexedPath<'a> {
    sequence_name: &'a str,
    index: usize,
    member: Option<&'a str>,
}

pub fn lower(
    program: &Program,
    runtime_model_templates: &HashMap<String, RuntimeModelTemplate>,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
) -> Result<Program, Vec<Diagnostic>> {
    let facts = ProgramFacts::from_program(program);
    let models = facts.models();
    let roots = facts.modeled_state_roots();
    let mut errors = Vec::new();

    reject_deferred_sequence_positions(program, &mut errors);
    let sequences = collect_sequences(program, models, roots, externalized_sequences, &mut errors);
    let filter_views = collect_filter_views(program);
    let designation_models = collect_designation_models(program);

    if !errors.is_empty() {
        return Err(errors);
    }

    let mut declarations = Vec::new();
    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        let context = LoweringContext {
            declaration_index,
            sequences: &sequences,
            filter_views: &filter_views,
            designation_models: &designation_models,
            models,
            runtime_model_templates,
        };

        match declaration {
            Declaration::State(state) => {
                if let Some(info) = sequences.get(&state.name) {
                    let initializer = lower_sequence_literal_for(
                        info,
                        &state.initializer,
                        state.location,
                        &mut errors,
                    );
                    declarations.push(Declaration::State(StateDecl {
                        location: state.location,
                        name: state.name.clone(),
                        type_name: Some("String".to_string()),
                        initializer,
                        implicit_model_initializer: false,
                    }));
                } else {
                    let mut lowered = state.clone();
                    lowered.initializer =
                        lower_expr(&state.initializer, state.location, &context, &mut errors);
                    declarations.push(Declaration::State(lowered));
                }
            }
            Declaration::Derived(derived) => declarations.push(Declaration::Derived(DerivedDecl {
                location: derived.location,
                name: derived.name.clone(),
                expression: lower_expr(
                    &derived.expression,
                    derived.location,
                    &context,
                    &mut errors,
                ),
            })),
            Declaration::Action(action) => declarations.push(Declaration::Action(ActionDecl {
                location: action.location,
                name: action.name.clone(),
                parameters: action.parameters.clone(),
                statements: action
                    .statements
                    .iter()
                    .map(|statement| lower_statement(statement, &context, &mut errors))
                    .collect(),
            })),
        }
    }

    if errors.is_empty() {
        Ok(Program {
            declarations,
            state_models: program.state_models.clone(),
        })
    } else {
        Err(errors)
    }
}

fn reject_deferred_sequence_positions(program: &Program, errors: &mut Vec<Diagnostic>) {
    for model in &program.state_models {
        for member in &model.members {
            if let StateModelMember::State(state) = member {
                if state
                    .type_name
                    .as_deref()
                    .and_then(decode_sequence_live_type)
                    .is_some()
                {
                    errors.push(diag(
                        state.location,
                        "bootstrap ordered-sequence spike supports sequence state only at top level; state-model sequence members are the next integration step",
                    ));
                }
            }
        }
    }

    for declaration in &program.declarations {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        for parameter in &action.parameters {
            if decode_sequence_live_type(&parameter.type_name).is_some() {
                errors.push(diag(
                    parameter.location,
                    "bootstrap ordered-sequence spike does not yet support sequence action parameters",
                ));
            }
        }
    }
}

fn collect_designation_models(program: &Program) -> HashMap<String, (String, usize)> {
    let mut result = HashMap::new();
    for (index, declaration) in program.declarations.iter().enumerate() {
        if let Declaration::State(state) = declaration {
            if let Some(type_name) = state.type_name.as_deref() {
                if let Some(model) = decode_live_type_name(type_name)
                    .or_else(|| decode_maybe_live_type_name(type_name))
                {
                    result.insert(state.name.clone(), (model.to_string(), index));
                }
            }
        }
        if let Declaration::Action(action) = declaration {
            for parameter in &action.parameters {
                if let Some(model) = decode_scope_identity_param(&parameter.name) {
                    result.insert(parameter.name.clone(), (model.to_string(), index));
                }
            }
        }
    }
    result
}

fn collect_filter_views(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::Derived(derived) = declaration else {
                return None;
            };
            if !derived.name.starts_with(MODEL_FILTER_BINDING_PREFIX) {
                return None;
            }
            let Expr::Filter {
                element_model: Some(element_model),
                ..
            } = &derived.expression
            else {
                return None;
            };
            Some((derived.name.clone(), element_model.clone()))
        })
        .collect()
}

fn collect_sequences(
    program: &Program,
    models: &HashMap<String, ModelInfo>,
    roots: &[RootInfo],
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, SequenceInfo> {
    let mut result = HashMap::new();

    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        let Declaration::State(state) = declaration else {
            continue;
        };
        let Some(model_name) = state
            .type_name
            .as_deref()
            .and_then(decode_sequence_live_type)
        else {
            continue;
        };

        if !models.contains_key(model_name) {
            errors.push(diag(
                state.location,
                format!("ordered sequence names unknown state model '{model_name}'"),
            ));
            continue;
        }

        let candidates: HashSet<String> = roots
            .iter()
            .filter(|root| {
                root.model_name == model_name && root.declaration_index < declaration_index
            })
            .map(|root| root.name.clone())
            .collect();

        let mut variants = Vec::new();
        if let Some(targets) = sequence_literal_targets(&state.initializer) {
            add_variant(
                &mut variants,
                targets,
                model_name,
                &candidates,
                state.location,
                errors,
            );
        } else {
            errors.push(diag(
                state.location,
                format!(
                    "ordered sequence state '{}' requires a sequence literal initializer in the bootstrap spike",
                    state.name
                ),
            ));
        }

        collect_assignment_variants(
            &program.declarations,
            &state.name,
            model_name,
            &candidates,
            &mut variants,
            errors,
        );

        let externalized = externalized_sequences.get(&state.name);
        if let Some(provenance) = externalized {
            if provenance.element_model != model_name {
                errors.push(diag(
                    state.location,
                    format!(
                        "internal model-sequence provenance mismatch for {}.{} on root {}: owner model {} records live {} but binding {} has live {}",
                        provenance.owner_model,
                        provenance.member_name,
                        provenance.owner_root,
                        provenance.owner_model,
                        provenance.element_model,
                        state.name,
                        model_name
                    ),
                ));
            }
        }

        result.insert(
            state.name.clone(),
            SequenceInfo {
                name: state.name.clone(),
                model_name: model_name.to_string(),
                declaration_index,
                candidates,
                variants,
                externalized_model_member: externalized.is_some(),
            },
        );
    }

    result
}

fn collect_assignment_variants(
    declarations: &[Declaration],
    sequence_name: &str,
    model_name: &str,
    candidates: &HashSet<String>,
    variants: &mut Vec<SequenceVariant>,
    errors: &mut Vec<Diagnostic>,
) {
    for declaration in declarations {
        if let Declaration::Action(action) = declaration {
            collect_variants_from_statements(
                &action.statements,
                sequence_name,
                model_name,
                candidates,
                variants,
                errors,
            );
        }
    }
}

fn collect_variants_from_statements(
    statements: &[Statement],
    sequence_name: &str,
    model_name: &str,
    candidates: &HashSet<String>,
    variants: &mut Vec<SequenceVariant>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Statement::Assignment {
                location,
                target,
                operator,
                value,
            } if target == sequence_name => {
                if *operator == AssignmentOperator::Assign {
                    if let Some(targets) = sequence_literal_targets(value) {
                        add_variant(variants, targets, model_name, candidates, *location, errors);
                    }
                }
            }
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_variants_from_statements(
                    then_branch,
                    sequence_name,
                    model_name,
                    candidates,
                    variants,
                    errors,
                );
                if let Some(else_branch) = else_branch {
                    collect_variants_from_statements(
                        else_branch,
                        sequence_name,
                        model_name,
                        candidates,
                        variants,
                        errors,
                    );
                }
            }
            _ => {}
        }
    }
}

fn add_variant(
    variants: &mut Vec<SequenceVariant>,
    targets: Vec<String>,
    model_name: &str,
    candidates: &HashSet<String>,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    for target in &targets {
        if !candidates.contains(target) {
            errors.push(diag(
                location,
                format!(
                    "'live {target}' is not an earlier declared live {model_name} target available to this ordered sequence"
                ),
            ));
        }
    }

    let encoded = encode_sequence_value(&targets);
    if !variants.iter().any(|variant| variant.encoded == encoded) {
        variants.push(SequenceVariant { targets, encoded });
    }
}

fn sequence_literal_targets(expression: &Expr) -> Option<Vec<String>> {
    let Expr::String(value) = expression else {
        return None;
    };
    decode_sequence_literal(value)
}

fn encode_sequence_value(targets: &[String]) -> String {
    format!("{LOWERED_SEQUENCE_VALUE_PREFIX}{}", targets.join("|"))
}

fn lower_sequence_literal_for(
    info: &SequenceInfo,
    expression: &Expr,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Some(targets) = sequence_literal_targets(expression) else {
        errors.push(diag(
            location,
            format!(
                "ordered sequence state '{}' requires a sequence literal value in the bootstrap spike",
                info.name
            ),
        ));
        return Expr::String(String::new());
    };
    if targets
        .iter()
        .any(|target| !info.candidates.contains(target))
    {
        errors.push(diag(
            location,
            format!(
                "ordered sequence '{}' contains a target outside its live {} candidate set",
                info.name, info.model_name
            ),
        ));
    }
    Expr::String(encode_sequence_value(&targets))
}

fn lower_expr(
    expression: &Expr,
    location: SourceLocation,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => {
            if decode_sequence_literal(value).is_some() {
                errors.push(diag(
                    location,
                    "ordered sequence literals are currently valid only as sequence state initializers or whole-sequence assignments",
                ));
                Expr::String(String::new())
            } else {
                Expr::String(value.clone())
            }
        }
        Expr::Name(name) => {
            if let Some(path) = decode_index_path(name) {
                if designation_sequence_member_model(path.sequence_name, context).is_some() {
                    let index = Expr::Integer(path.index as i64);
                    return match path.member {
                        Some(member) => lower_runtime_indexed_member_read(
                            path.sequence_name,
                            &index,
                            member,
                            location,
                            context,
                            errors,
                        ),
                        None => lower_runtime_indexed_designation(
                            path.sequence_name,
                            &index,
                            location,
                            context,
                            errors,
                        ),
                    };
                }

                let Some(info) = context.sequences.get(path.sequence_name) else {
                    errors.push(diag(
                        location,
                        format!("'{}' is not an ordered sequence state", path.sequence_name),
                    ));
                    return Expr::Integer(0);
                };
                if info.declaration_index >= context.declaration_index {
                    errors.push(diag(
                        location,
                        format!(
                            "ordered sequence '{}' is not available before its declaration",
                            path.sequence_name
                        ),
                    ));
                }
                let Some(member) = path.member else {
                    if info.externalized_model_member {
                        return Expr::RuntimeIndexDesignation {
                            source: Box::new(Expr::Name(info.name.clone())),
                            index: Box::new(Expr::Integer(path.index as i64)),
                            element_model: info.model_name.clone(),
                        };
                    }
                    errors.push(diag(
                        location,
                        "reading an indexed live designation as an ordinary whole value is deferred for static sequence state",
                    ));
                    return Expr::Integer(0);
                };
                return lower_indexed_member_read(
                    info, path.index, member, location, context, errors,
                );
            }

            if context.sequences.contains_key(name) {
                errors.push(diag(
                    location,
                    format!(
                        "ordered sequence '{}' is a structural value whose whole-value projection is deferred in the bootstrap spike",
                        name
                    ),
                ));
                return Expr::String(String::new());
            }
            Expr::Name(name.clone())
        }
        Expr::Binary {
            operator,
            left,
            right,
        } if matches!(
            *operator,
            BinaryOperator::PreviousIn | BinaryOperator::NextIn
        ) =>
        {
            let direction = if *operator == BinaryOperator::NextIn {
                "next"
            } else {
                "previous"
            };
            let Expr::Name(designation_name) = left.as_ref() else {
                errors.push(diag(
                    location,
                    format!(
                        "{direction} relative navigation requires a persistent live designation anchor"
                    ),
                ));
                return Expr::String(String::new());
            };
            let Some((designation_model, designation_index)) =
                context.designation_models.get(designation_name)
            else {
                errors.push(diag(
                    location,
                    format!(
                        "relative-navigation anchor '{}' is not a persistent live designation",
                        designation_name
                    ),
                ));
                return Expr::String(String::new());
            };
            if decode_scope_identity_param(designation_name).is_none()
                && *designation_index >= context.declaration_index
            {
                errors.push(diag(
                    location,
                    format!(
                        "live designation '{}' is not available before its declaration",
                        designation_name
                    ),
                ));
                return Expr::String(String::new());
            }

            let Expr::Name(source_name) = right.as_ref() else {
                errors.push(diag(
                    location,
                    "relative-navigation source must be one stored [live T] membership or direct filtered view",
                ));
                return Expr::String(String::new());
            };
            let dynamic_source_model = designation_sequence_member_model(source_name, context);
            let source_model = context
                .sequences
                .get(source_name)
                .map(|info| info.model_name.as_str())
                .or_else(|| context.filter_views.get(source_name).map(String::as_str))
                .or(dynamic_source_model.as_deref());
            let Some(source_model) = source_model else {
                errors.push(diag(
                    location,
                    "relative-navigation source must be one supported [live T] membership or direct filtered view",
                ));
                return Expr::String(String::new());
            };
            if designation_model != source_model {
                errors.push(diag(
                    location,
                    format!(
                        "relative-navigation anchor '{}' has type live {} but source contains live {}",
                        designation_name, designation_model, source_model
                    ),
                ));
                return Expr::String(String::new());
            }

            Expr::Binary {
                operator: *operator,
                left: left.clone(),
                right: right.clone(),
            }
        }
        Expr::Binary {
            operator: BinaryOperator::IsIn,
            left,
            right,
        } => {
            let Expr::Name(designation_name) = left.as_ref() else {
                errors.push(diag(
                    location,
                    "left operand of 'is in' must be a persistent live designation",
                ));
                return Expr::Bool(false);
            };
            let Some((designation_model, designation_index)) =
                context.designation_models.get(designation_name)
            else {
                errors.push(diag(
                    location,
                    format!(
                        "left operand '{}' of 'is in' is not a persistent live designation",
                        designation_name
                    ),
                ));
                return Expr::Bool(false);
            };
            if decode_scope_identity_param(designation_name).is_none()
                && *designation_index >= context.declaration_index
            {
                errors.push(diag(
                    location,
                    format!(
                        "live designation '{}' is not available before its declaration",
                        designation_name
                    ),
                ));
                return Expr::Bool(false);
            }

            let Expr::Name(source_name) = right.as_ref() else {
                errors.push(diag(
                    location,
                    "right operand of 'is in' must be one stored [live T] membership or direct filtered view",
                ));
                return Expr::Bool(false);
            };
            let dynamic_source_model = designation_sequence_member_model(source_name, context);
            let source_model = context
                .sequences
                .get(source_name)
                .map(|info| info.model_name.as_str())
                .or_else(|| context.filter_views.get(source_name).map(String::as_str))
                .or(dynamic_source_model.as_deref());
            let Some(source_model) = source_model else {
                errors.push(diag(
                    location,
                    "right operand of 'is in' must be one supported [live T] membership or direct filtered view",
                ));
                return Expr::Bool(false);
            };
            if designation_model != source_model {
                errors.push(diag(
                    location,
                    format!(
                        "live designation '{}' has type live {} but 'is in' view contains live {}",
                        designation_name, designation_model, source_model
                    ),
                ));
                return Expr::Bool(false);
            }

            Expr::Binary {
                operator: BinaryOperator::IsIn,
                left: left.clone(),
                right: right.clone(),
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(lower_expr(left, location, context, errors)),
            right: Box::new(lower_expr(right, location, context, errors)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_expr(condition, location, context, errors)),
            then_branch: Box::new(lower_expr(then_branch, location, context, errors)),
            else_branch: Box::new(lower_expr(else_branch, location, context, errors)),
        },
        Expr::IndexedMember {
            source,
            index,
            member,
        } => lower_runtime_indexed_member_read(source, index, member, location, context, errors),
        Expr::IndexedDesignation { source, index } => {
            lower_runtime_indexed_designation(source, index, location, context, errors)
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => expression.clone(),
        Expr::Filter { .. } => expression.clone(),
    }
}

fn lower_indexed_member_read(
    info: &SequenceInfo,
    index: usize,
    member: &str,
    location: SourceLocation,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if info.externalized_model_member {
        let Some(template) = context.runtime_model_templates.get(&info.model_name) else {
            errors.push(diag(
                location,
                format!("missing runtime model template for '{}'", info.model_name),
            ));
            return Expr::Integer(0);
        };
        let Some(member_template) = template.member(member) else {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    info.model_name, member
                ),
            ));
            return Expr::Integer(0);
        };

        return Expr::RuntimeIndexMember {
            source: Box::new(Expr::Name(info.name.clone())),
            index: Box::new(Expr::Integer(index as i64)),
            member: member.to_string(),
            element_model: info.model_name.clone(),
            member_type_name: runtime_value_type_name(&member_template.value_type),
        };
    }

    let Some(model) = context.models.get(&info.model_name) else {
        return Expr::Integer(0);
    };
    if !model.members.contains_key(member) {
        errors.push(diag(
            location,
            format!(
                "state model '{}' has no member '{}'",
                info.model_name, member
            ),
        ));
        return Expr::Integer(0);
    }

    let Some(branches) = variant_roots_at(info, index, location, errors) else {
        return Expr::Integer(0);
    };
    let expressions = branches
        .iter()
        .map(|(_, root)| Expr::Name(format!("{root}.{member}")))
        .collect();
    build_expr_choice(info, branches, expressions)
}

fn designation_sequence_member_source(
    source: &str,
    context: &LoweringContext<'_>,
) -> Option<(Expr, String)> {
    let (designation, member) = source.split_once('.')?;
    if designation.is_empty() || member.is_empty() || member.contains('.') {
        return None;
    }

    let (owner_model, _) = context.designation_models.get(designation)?;
    let member_template = context
        .runtime_model_templates
        .get(owner_model)?
        .member(member)?;

    let ValueType::SequenceLive(element_model) = &member_template.value_type else {
        return None;
    };

    Some((
        Expr::RuntimeDesignationMember {
            designation: Box::new(Expr::Name(designation.to_string())),
            member: member.to_string(),
            element_model: owner_model.clone(),
            member_type_name: runtime_value_type_name(&member_template.value_type),
        },
        element_model.clone(),
    ))
}

fn designation_sequence_member_model(
    source: &str,
    context: &LoweringContext<'_>,
) -> Option<String> {
    designation_sequence_member_source(source, context).map(|(_, model)| model)
}

fn lower_runtime_indexed_designation(
    source: &str,
    index: &Expr,
    location: SourceLocation,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if let Some(element_model) = context.filter_views.get(source) {
        return Expr::RuntimeIndexDesignation {
            source: Box::new(Expr::Name(source.to_string())),
            index: Box::new(lower_expr(index, location, context, errors)),
            element_model: element_model.clone(),
        };
    }
    if let Some((source, element_model)) = designation_sequence_member_source(source, context) {
        return Expr::RuntimeIndexDesignation {
            source: Box::new(source),
            index: Box::new(lower_expr(index, location, context, errors)),
            element_model,
        };
    }
    let Some(info) = context.sequences.get(source) else {
        errors.push(diag(
            location,
            format!("'{source}' is not an ordered sequence state or supported filtered view"),
        ));
        return Expr::String(String::new());
    };
    if !info.externalized_model_member {
        errors.push(diag(
            location,
            "runtime whole-designation indexing is currently supported only for owner-relative runtime-sized membership",
        ));
        return Expr::String(String::new());
    }
    Expr::RuntimeIndexDesignation {
        source: Box::new(Expr::Name(info.name.clone())),
        index: Box::new(lower_expr(index, location, context, errors)),
        element_model: info.model_name.clone(),
    }
}

fn lower_runtime_indexed_member_read(
    source: &str,
    index: &Expr,
    member: &str,
    location: SourceLocation,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if let Some(element_model) = context.filter_views.get(source) {
        let Some(template) = context.runtime_model_templates.get(element_model) else {
            errors.push(diag(
                location,
                format!("missing runtime model template for '{element_model}'"),
            ));
            return Expr::Integer(0);
        };
        let Some(member_template) = template.member(member) else {
            errors.push(diag(
                location,
                format!("state model '{element_model}' has no member '{member}'"),
            ));
            return Expr::Integer(0);
        };
        return Expr::RuntimeIndexMember {
            source: Box::new(Expr::Name(source.to_string())),
            index: Box::new(lower_expr(index, location, context, errors)),
            member: member.to_string(),
            element_model: element_model.clone(),
            member_type_name: runtime_value_type_name(&member_template.value_type),
        };
    }

    if let Some((source, element_model)) = designation_sequence_member_source(source, context) {
        let Some(template) = context.runtime_model_templates.get(&element_model) else {
            errors.push(diag(
                location,
                format!("missing runtime model template for '{element_model}'"),
            ));
            return Expr::Integer(0);
        };
        let Some(member_template) = template.member(member) else {
            errors.push(diag(
                location,
                format!("state model '{element_model}' has no member '{member}'"),
            ));
            return Expr::Integer(0);
        };
        return Expr::RuntimeIndexMember {
            source: Box::new(source),
            index: Box::new(lower_expr(index, location, context, errors)),
            member: member.to_string(),
            element_model,
            member_type_name: runtime_value_type_name(&member_template.value_type),
        };
    }
    let Some(info) = context.sequences.get(source) else {
        errors.push(diag(
            location,
            format!("'{source}' is not an ordered sequence state or supported filtered view"),
        ));
        return Expr::Integer(0);
    };
    if !info.externalized_model_member {
        errors.push(diag(
            location,
            "runtime index expressions are currently supported only for owner-relative state-model sequence members",
        ));
        return Expr::Integer(0);
    }
    let Some(template) = context.runtime_model_templates.get(&info.model_name) else {
        errors.push(diag(
            location,
            format!("missing runtime model template for '{}'", info.model_name),
        ));
        return Expr::Integer(0);
    };
    let Some(member_template) = template.member(member) else {
        errors.push(diag(
            location,
            format!(
                "state model '{}' has no member '{}'",
                info.model_name, member
            ),
        ));
        return Expr::Integer(0);
    };
    Expr::RuntimeIndexMember {
        source: Box::new(Expr::Name(info.name.clone())),
        index: Box::new(lower_expr(index, location, context, errors)),
        member: member.to_string(),
        element_model: info.model_name.clone(),
        member_type_name: runtime_value_type_name(&member_template.value_type),
    }
}

fn lower_statement(
    statement: &Statement,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => {
            if let Some(path) = decode_through_path(target).and_then(decode_index_path) {
                let lowered_value = lower_expr(value, *location, context, errors);
                return lower_through_assignment(
                    path,
                    *location,
                    *operator,
                    lowered_value,
                    context,
                    errors,
                );
            }

            if let Some(info) = context.sequences.get(target) {
                if *operator != AssignmentOperator::Assign {
                    errors.push(diag(
                        *location,
                        format!(
                            "ordered sequence state '{}' supports whole-value replacement only in the bootstrap spike",
                            target
                        ),
                    ));
                }
                return Statement::Assignment {
                    location: *location,
                    target: target.clone(),
                    operator: AssignmentOperator::Assign,
                    value: lower_sequence_literal_for(info, value, *location, errors),
                };
            }

            if decode_index_path(target).is_some() {
                errors.push(diag(
                    *location,
                    "sequence positions are values, not independently writable state; indirect target mutation requires 'through'",
                ));
                return statement.clone();
            }

            Statement::Assignment {
                location: *location,
                target: target.clone(),
                operator: *operator,
                value: lower_expr(value, *location, context, errors),
            }
        }
        Statement::IndexedThroughAssignment {
            location,
            source,
            index,
            member,
            operator,
            value,
        } => lower_runtime_indexed_through_assignment(
            source,
            index,
            member,
            *location,
            *operator,
            lower_expr(value, *location, context, errors),
            context,
            errors,
        ),
        Statement::RuntimeIndexAssignment { .. } => statement.clone(),
        Statement::RuntimeDesignationAssignment { .. } => {
            unreachable!("runtime designation lowering runs after sequence lowering")
        }
        Statement::ActionCall {
            location,
            name,
            arguments,
        } => expand_action_call(*location, name, arguments, context, errors),
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: lower_expr(message, *location, context, errors),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: lower_expr(condition, *location, context, errors),
            then_branch: then_branch
                .iter()
                .map(|statement| lower_statement(statement, context, errors))
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| lower_statement(statement, context, errors))
                    .collect()
            }),
        },
    }
}

fn lower_through_assignment(
    path: IndexedPath<'_>,
    location: SourceLocation,
    operator: AssignmentOperator,
    value: Expr,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    let Some(member) = path.member else {
        errors.push(diag(
            location,
            "direct 'through' assignment requires an indexed designation member",
        ));
        return Statement::Assignment {
            location,
            target: path.sequence_name.to_string(),
            operator,
            value,
        };
    };
    let Some(info) = context.sequences.get(path.sequence_name) else {
        errors.push(diag(
            location,
            format!("'{}' is not an ordered sequence state", path.sequence_name),
        ));
        return Statement::Assignment {
            location,
            target: path.sequence_name.to_string(),
            operator,
            value,
        };
    };

    if info.externalized_model_member {
        let Some(template) = context.runtime_model_templates.get(&info.model_name) else {
            errors.push(diag(
                location,
                format!("missing runtime model template for '{}'", info.model_name),
            ));
            return Statement::Assignment {
                location,
                target: path.sequence_name.to_string(),
                operator,
                value,
            };
        };
        let Some(member_template) = template.member(member) else {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    info.model_name, member
                ),
            ));
            return Statement::Assignment {
                location,
                target: path.sequence_name.to_string(),
                operator,
                value,
            };
        };
        if member_template.kind != RuntimeModelMemberKind::State {
            errors.push(diag(
                location,
                format!(
                    "cannot mutate derived member '{}' through sequence designation",
                    member
                ),
            ));
        }

        return Statement::RuntimeIndexAssignment {
            location,
            source: info.name.clone(),
            index: Box::new(Expr::Integer(path.index as i64)),
            member: member.to_string(),
            element_model: info.model_name.clone(),
            member_type_name: runtime_value_type_name(&member_template.value_type),
            operator,
            value,
        };
    }

    let Some(model) = context.models.get(&info.model_name) else {
        return Statement::Assignment {
            location,
            target: path.sequence_name.to_string(),
            operator,
            value,
        };
    };
    match model.members.get(member) {
        Some(MemberKind::State) => {}
        Some(MemberKind::Derived) => errors.push(diag(
            location,
            format!(
                "cannot mutate derived member '{}' through sequence designation",
                member
            ),
        )),
        None => errors.push(diag(
            location,
            format!(
                "state model '{}' has no member '{}'",
                info.model_name, member
            ),
        )),
    }

    let Some(branches) = variant_roots_at(info, path.index, location, errors) else {
        return Statement::Assignment {
            location,
            target: path.sequence_name.to_string(),
            operator,
            value,
        };
    };
    let statements = branches
        .iter()
        .map(|(_, root)| Statement::Assignment {
            location,
            target: format!("{root}.{member}"),
            operator,
            value: value.clone(),
        })
        .collect();
    build_statement_choice(info, location, branches, statements)
}

#[allow(clippy::too_many_arguments)]
fn lower_runtime_indexed_through_assignment(
    source: &str,
    index: &Expr,
    member: &str,
    location: SourceLocation,
    operator: AssignmentOperator,
    value: Expr,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    if let Some(element_model) = context.filter_views.get(source) {
        let Some(template) = context.runtime_model_templates.get(element_model) else {
            errors.push(diag(
                location,
                format!("missing runtime model template for '{element_model}'"),
            ));
            return Statement::IndexedThroughAssignment {
                location,
                source: source.to_string(),
                index: index.clone(),
                member: member.to_string(),
                operator,
                value,
            };
        };
        let Some(member_template) = template.member(member) else {
            errors.push(diag(
                location,
                format!("state model '{element_model}' has no member '{member}'"),
            ));
            return Statement::IndexedThroughAssignment {
                location,
                source: source.to_string(),
                index: index.clone(),
                member: member.to_string(),
                operator,
                value,
            };
        };
        if member_template.kind != RuntimeModelMemberKind::State {
            errors.push(diag(
                location,
                format!(
                    "cannot mutate derived member '{}' through sequence designation",
                    member
                ),
            ));
        }
        return Statement::RuntimeIndexAssignment {
            location,
            source: source.to_string(),
            index: Box::new(lower_expr(index, location, context, errors)),
            member: member.to_string(),
            element_model: element_model.clone(),
            member_type_name: runtime_value_type_name(&member_template.value_type),
            operator,
            value,
        };
    }

    let Some(info) = context.sequences.get(source) else {
        errors.push(diag(
            location,
            format!("'{source}' is not an ordered sequence state or supported filtered view"),
        ));
        return Statement::IndexedThroughAssignment {
            location,
            source: source.to_string(),
            index: index.clone(),
            member: member.to_string(),
            operator,
            value,
        };
    };
    if !info.externalized_model_member {
        errors.push(diag(
            location,
            "runtime index expressions are currently supported only for owner-relative state-model sequence members",
        ));
        return Statement::IndexedThroughAssignment {
            location,
            source: source.to_string(),
            index: index.clone(),
            member: member.to_string(),
            operator,
            value,
        };
    }
    let Some(template) = context.runtime_model_templates.get(&info.model_name) else {
        errors.push(diag(
            location,
            format!("missing runtime model template for '{}'", info.model_name),
        ));
        return Statement::IndexedThroughAssignment {
            location,
            source: source.to_string(),
            index: index.clone(),
            member: member.to_string(),
            operator,
            value,
        };
    };
    let Some(member_template) = template.member(member) else {
        errors.push(diag(
            location,
            format!(
                "state model '{}' has no member '{}'",
                info.model_name, member
            ),
        ));
        return Statement::IndexedThroughAssignment {
            location,
            source: source.to_string(),
            index: index.clone(),
            member: member.to_string(),
            operator,
            value,
        };
    };
    if member_template.kind != RuntimeModelMemberKind::State {
        errors.push(diag(
            location,
            format!(
                "cannot mutate derived member '{}' through sequence designation",
                member
            ),
        ));
    }
    Statement::RuntimeIndexAssignment {
        location,
        source: info.name.clone(),
        index: Box::new(lower_expr(index, location, context, errors)),
        member: member.to_string(),
        element_model: info.model_name.clone(),
        member_type_name: runtime_value_type_name(&member_template.value_type),
        operator,
        value,
    }
}

fn expand_action_call(
    location: SourceLocation,
    name: &str,
    arguments: &[ActionArgument],
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    let indexed_through = arguments.iter().enumerate().find_map(|(index, argument)| {
        let ActionArgument::StateGrant { location, name } = argument else {
            return None;
        };
        let path = decode_through_path(name)?;
        let decoded = decode_index_path(path)?;
        Some((index, *location, decoded))
    });

    if let Some((argument_index, grant_location, path)) = indexed_through {
        let Some(info) = context.sequences.get(path.sequence_name) else {
            errors.push(diag(
                grant_location,
                format!("'{}' is not an ordered sequence state", path.sequence_name),
            ));
            return Statement::ActionCall {
                location,
                name: name.to_string(),
                arguments: arguments.to_vec(),
            };
        };

        if let Some(member) = path.member {
            let Some(model) = context.models.get(&info.model_name) else {
                return Statement::ActionCall {
                    location,
                    name: name.to_string(),
                    arguments: arguments.to_vec(),
                };
            };
            match model.members.get(member) {
                Some(MemberKind::State) => {}
                Some(MemberKind::Derived) => errors.push(diag(
                    grant_location,
                    format!("cannot grant derived member '{}' as writable state", member),
                )),
                None => errors.push(diag(
                    grant_location,
                    format!(
                        "state model '{}' has no member '{}'",
                        info.model_name, member
                    ),
                )),
            }
        }

        let Some(branches) = variant_roots_at(info, path.index, grant_location, errors) else {
            return Statement::ActionCall {
                location,
                name: name.to_string(),
                arguments: arguments.to_vec(),
            };
        };

        let statements = branches
            .iter()
            .map(|(_, root)| {
                let mut replaced = arguments.to_vec();
                let resolved = path
                    .member
                    .map(|member| format!("{root}.{member}"))
                    .unwrap_or_else(|| root.clone());
                replaced[argument_index] = ActionArgument::StateGrant {
                    location: grant_location,
                    name: resolved,
                };
                expand_action_call(location, name, &replaced, context, errors)
            })
            .collect();
        return build_statement_choice(info, location, branches, statements);
    }

    let arguments = arguments
        .iter()
        .map(|argument| match argument {
            ActionArgument::Value(expression) => {
                ActionArgument::Value(lower_expr(expression, location, context, errors))
            }
            ActionArgument::IndexedStateGrant { .. } => {
                unreachable!("runtime index grant transport runs before sequence lowering")
            }
            ActionArgument::StateGrant {
                location: grant_location,
                name: granted,
            } => {
                if context.sequences.contains_key(granted)
                    && !name.starts_with(crate::scoped_create_surface::GENERATED_INSERT_ACTION_PREFIX)
                    && !name.starts_with(
                        crate::structural_edit_surface::GENERATED_REMOVE_ACTION_PREFIX,
                    )
                    && !name.starts_with(
                        crate::structural_edit_surface::GENERATED_FILTERED_REMOVE_ACTION_PREFIX,
                    )
                    && !name.starts_with(
                        crate::structural_move_surface::GENERATED_MOVE_ACTION_PREFIX,
                    )
                    && !name.starts_with(
                        crate::structural_move_surface::GENERATED_FILTERED_MOVE_ACTION_PREFIX,
                    )
                {
                    errors.push(diag(
                        *grant_location,
                        "whole ordered-sequence authority cannot be passed to an action until sequence action parameters are defined",
                    ));
                } else if decode_index_path(granted).is_some() {
                    errors.push(diag(
                        *grant_location,
                        "sequence positions are values; target authority through an indexed designation requires 'state through'",
                    ));
                }
                ActionArgument::StateGrant {
                    location: *grant_location,
                    name: granted.clone(),
                }
            }
        })
        .collect();

    Statement::ActionCall {
        location,
        name: name.to_string(),
        arguments,
    }
}

fn variant_roots_at<'a>(
    info: &'a SequenceInfo,
    index: usize,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<(&'a SequenceVariant, String)>> {
    if info.variants.is_empty() {
        errors.push(diag(
            location,
            format!("ordered sequence '{}' has no known values", info.name),
        ));
        return None;
    }

    let mut result = Vec::new();
    for variant in &info.variants {
        let Some(root) = variant.targets.get(index) else {
            errors.push(diag(
                location,
                format!(
                    "index {} is not valid for every known value of ordered sequence '{}' in the bootstrap spike",
                    index, info.name
                ),
            ));
            return None;
        };
        result.push((variant, root.clone()));
    }
    Some(result)
}

fn build_expr_choice(
    info: &SequenceInfo,
    branches: Vec<(&SequenceVariant, String)>,
    expressions: Vec<Expr>,
) -> Expr {
    debug_assert_eq!(branches.len(), expressions.len());
    let mut paired: Vec<_> = branches.into_iter().zip(expressions).collect();
    let ((_, _), mut expression) = paired
        .pop()
        .expect("sequence choice requires at least one known value");
    for ((variant, _), branch) in paired.into_iter().rev() {
        expression = Expr::If {
            condition: Box::new(sequence_condition(info, variant)),
            then_branch: Box::new(branch),
            else_branch: Box::new(expression),
        };
    }
    expression
}

fn build_statement_choice(
    info: &SequenceInfo,
    location: SourceLocation,
    branches: Vec<(&SequenceVariant, String)>,
    statements: Vec<Statement>,
) -> Statement {
    debug_assert_eq!(branches.len(), statements.len());
    let mut paired: Vec<_> = branches.into_iter().zip(statements).collect();
    let ((_, _), mut statement) = paired
        .pop()
        .expect("sequence choice requires at least one known value");
    for ((variant, _), branch) in paired.into_iter().rev() {
        statement = Statement::If {
            location,
            condition: sequence_condition(info, variant),
            then_branch: vec![branch],
            else_branch: Some(vec![statement]),
        };
    }
    statement
}

fn sequence_condition(info: &SequenceInfo, variant: &SequenceVariant) -> Expr {
    Expr::Binary {
        operator: BinaryOperator::Equal,
        left: Box::new(Expr::Name(info.name.clone())),
        right: Box::new(Expr::String(variant.encoded.clone())),
    }
}

fn decode_index_path(path: &str) -> Option<IndexedPath<'_>> {
    let marker = format!(".{SEQUENCE_INDEX_SEGMENT_PREFIX}");
    let (sequence_name, indexed_suffix) = path.split_once(&marker)?;
    if sequence_name.is_empty() {
        return None;
    }

    let (index_text, member) = match indexed_suffix.split_once('.') {
        Some((index, member)) if !member.is_empty() && !member.contains('.') => {
            (index, Some(member))
        }
        Some(_) => return None,
        None => (indexed_suffix, None),
    };
    let index = index_text.parse().ok()?;

    Some(IndexedPath {
        sequence_name,
        index,
        member,
    })
}

fn runtime_value_type_name(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Int => "Int".to_string(),
        ValueType::Float => "Float".to_string(),
        ValueType::Bool => "Bool".to_string(),
        ValueType::String => "String".to_string(),
        ValueType::SequenceLive(model) => encode_runtime_sequence_type(model),
        ValueType::Named(name) => name.clone(),
    }
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
