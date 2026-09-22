pub mod ast;
mod create_surface;
mod designation_runtime_metadata;
pub mod diagnostic;
#[cfg(test)]
mod dynamic_owner_structural_edit_tests;
mod existing_designation_insert_surface;
mod filter_integration;
pub mod lexer;
mod lifetime_termination_surface;
mod lifetime_transfer_surface;
pub mod live_designation_lowering;
pub mod model_lowering;
pub mod model_sequence_integration;
#[cfg(test)]
mod model_sequence_provenance_tests;
mod model_types;
#[cfg(test)]
mod nested_folder_composition_tests;
pub mod parser;
mod program_facts;
pub mod reduction_integration;
pub mod reduction_lowering;
pub mod reduction_surface;
mod relative_navigation;
#[cfg(test)]
mod relative_navigation_pressure;
pub mod runtime;
mod runtime_index_grant_transport;
pub mod runtime_model_templates;
mod runtime_reduction_typing;
pub mod runtime_sequence_markers;
pub mod runtime_sequence_realization;
mod scoped_create_surface;
mod scoped_designation_surface;
mod scoped_designation_validation;
pub mod semantic;
pub mod sequence_lowering;
pub mod sequence_surface;
mod structural_edit_surface;
mod structural_move;
mod structural_move_surface;
pub mod token;

use std::collections::HashMap;

use ast::Program;
use designation_runtime_metadata::RuntimeDesignationMetadata;
use diagnostic::Diagnostic;
use runtime_model_templates::{RuntimeModelRoot, RuntimeModelTemplate};
use semantic::CheckedProgram;

#[derive(Debug, Clone)]
pub struct CheckedSource {
    pub program: CheckedProgram,
    pub runtime_model_templates: HashMap<String, RuntimeModelTemplate>,
    pub runtime_model_roots: HashMap<String, RuntimeModelRoot>,
    pub(crate) runtime_designations: HashMap<String, RuntimeDesignationMetadata>,
}

pub fn parse_source(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let source = lifetime_transfer_surface::preprocess(source)?;
    let source = lifetime_termination_surface::preprocess(&source)?;
    let source = structural_edit_surface::preprocess(&source)?;
    let source = structural_move_surface::preprocess(&source)?;
    let source = scoped_create_surface::preprocess(&source)?;
    let source = scoped_designation_surface::preprocess(&source)?;
    let source = create_surface::preprocess(&source)?;
    let source = reduction_surface::preprocess(&source)?;
    let source = sequence_surface::preprocess(&source)?;
    let tokens = lexer::lex(&source)?;
    parser::Parser::new(tokens).parse_program()
}

pub fn check_source(source: &str) -> Result<CheckedProgram, Vec<Diagnostic>> {
    check_source_with_runtime_models(source).map(|checked| checked.program)
}

pub fn check_source_with_runtime_models(source: &str) -> Result<CheckedSource, Vec<Diagnostic>> {
    let program = parse_source(source)?;
    let runtime_model_templates = runtime_model_templates::collect(&program.state_models)?;
    let runtime_model_roots =
        runtime_model_templates::collect_roots(&program, &runtime_model_templates);
    lifetime_transfer_surface::validate(&program, &runtime_model_roots)?;
    lifetime_termination_surface::validate(&program, &runtime_model_roots)?;
    create_surface::validate(&program, &runtime_model_templates, &runtime_model_roots)?;
    scoped_create_surface::validate(&program, &runtime_model_templates, &runtime_model_roots)?;
    let lifetime_transfer_prepared =
        lifetime_transfer_surface::lower_owners(&program, &runtime_model_roots);
    let lifetime_owner_prepared = lifetime_termination_surface::lower_owners(
        &lifetime_transfer_prepared,
        &runtime_model_roots,
    );
    let scoped_create_surface::ScopedCreateLowering {
        program: create_prepared,
        designation_models: create_scope_designations,
    } = scoped_create_surface::lower_scopes(
        &lifetime_owner_prepared,
        &runtime_model_templates,
        &runtime_model_roots,
    )?;
    let scoped_designation_prepared = scoped_designation_surface::lower_scopes(
        &create_prepared,
        &runtime_model_templates,
        &runtime_model_roots,
    )?;
    scoped_designation_validation::validate(&scoped_designation_prepared)?;
    let existing_insert_prepared = existing_designation_insert_surface::lower(
        &scoped_designation_prepared,
        &runtime_model_templates,
        &runtime_model_roots,
    )?;
    let structural_move_prepared = structural_move_surface::lower(
        &existing_insert_prepared,
        &runtime_model_templates,
        &runtime_model_roots,
    )?;
    let structural_edit_prepared = structural_edit_surface::lower(
        &structural_move_prepared,
        &runtime_model_templates,
        &runtime_model_roots,
    )?;
    let reduction_prepared = reduction_integration::lower(&structural_edit_prepared)?;
    let filter_prepared = filter_integration::lower(&reduction_prepared)?;
    let model_sequence_integration::ModelSequenceLowering {
        program: model_sequence_lowered,
        externalized_sequences,
    } = model_sequence_integration::lower(&filter_prepared)?;
    let runtime_index_grants =
        runtime_index_grant_transport::lower(&model_sequence_lowered, &runtime_model_templates)?;
    let sequence_lowered = sequence_lowering::lower(
        &runtime_index_grants,
        &runtime_model_templates,
        &externalized_sequences,
    )?;

    // Preserve owner-relative model-sequence reductions for structural runtime
    // execution while lowering every other reduction through the existing
    // bootstrap path.
    let runtime_reduction_lowered =
        reduction_lowering::lower_preserving_model_sequences(&sequence_lowered)?;

    // Type owner-relative reductions directly from the stable initial-value
    // accumulator law plus owner/element model schemas. Static typing no longer
    // depends on choosing a representative concrete live target.
    let runtime_reduction_types = runtime_reduction_typing::check(&runtime_reduction_lowered)?;
    let runtime_designations = designation_runtime_metadata::collect(&runtime_reduction_lowered);

    let runtime_live_lowered = live_designation_lowering::lower(
        &runtime_reduction_lowered,
        &runtime_model_templates,
        &create_scope_designations,
    )?;
    let existing_insert_finalized =
        existing_designation_insert_surface::finalize(&runtime_live_lowered)?;
    let runtime_lowered = model_lowering::lower(&existing_insert_finalized)?;
    let runtime_realized = runtime_sequence_realization::lower(
        &runtime_lowered,
        &runtime_reduction_types,
        &externalized_sequences,
    )?;
    let checked = semantic::check_with_runtime_reductions(
        &runtime_realized.program,
        runtime_realized.runtime_reductions,
    )?;

    Ok(CheckedSource {
        program: checked,
        runtime_model_templates,
        runtime_model_roots,
        runtime_designations,
    })
}
