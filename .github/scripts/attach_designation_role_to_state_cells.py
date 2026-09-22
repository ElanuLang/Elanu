from pathlib import Path

path = Path("compiler/src/runtime.rs")
text = path.read_text()

replacements = [
    (
        '''struct StateCell {
    value: Value,
    value_type: ValueType,
    dependents: HashSet<String>,
}
''',
        '''struct StateCell {
    value: Value,
    value_type: ValueType,
    designation: Option<RuntimeDesignationMetadata>,
    dependents: HashSet<String>,
}
''',
        "StateCell designation role",
    ),
    (
        '''                        StateCell {
                            value,
                            value_type,
                            dependents: HashSet::new(),
                        },
''',
        '''                        StateCell {
                            value,
                            value_type,
                            designation: None,
                            dependents: HashSet::new(),
                        },
''',
        "top-level StateCell construction",
    ),
    (
        '''            let cell = StateCell {
                value,
                value_type: member.value_type.clone(),
                dependents: HashSet::new(),
            };
''',
        '''            let cell = StateCell {
                value,
                value_type: member.value_type.clone(),
                designation: None,
                dependents: HashSet::new(),
            };
''',
        "dynamic model StateCell construction",
    ),
    (
        '''    pub fn from_checked_source(source: &CheckedSource) -> Result<Self, RuntimeError> {
        let mut runtime = Self::from_program(&source.program)?;
        runtime.runtime_model_templates = source.runtime_model_templates.clone();
        runtime.runtime_model_roots = source.runtime_model_roots.clone();
        runtime.runtime_designations = source.runtime_designations.clone();
        Ok(runtime)
    }
''',
        '''    pub fn from_checked_source(source: &CheckedSource) -> Result<Self, RuntimeError> {
        let mut runtime = Self::from_program(&source.program)?;
        runtime.runtime_model_templates = source.runtime_model_templates.clone();
        runtime.runtime_model_roots = source.runtime_model_roots.clone();
        runtime.runtime_designations = source.runtime_designations.clone();
        for (name, metadata) in &source.runtime_designations {
            let state = runtime.states.get_mut(name).ok_or_else(|| {
                RuntimeError::new(format!(
                    "persistent designation metadata names missing state '{name}'"
                ))
            })?;
            state.designation = Some(metadata.clone());
        }
        Ok(runtime)
    }
''',
        "top-level designation role attachment",
    ),
    (
        '''        let designation_metadata = self
            .runtime_designations
            .iter()
            .map(|(name, metadata)| (name.clone(), metadata.allows_none))
            .collect::<Vec<_>>();
''',
        '''        let designation_deleted_states = self
            .transaction
            .as_ref()
            .expect("transaction should exist")
            .deleted_states
            .clone();
        let mut designation_metadata = self
            .states
            .iter()
            .filter(|(name, _)| !designation_deleted_states.contains(*name))
            .filter_map(|(name, cell)| {
                cell.designation
                    .as_ref()
                    .map(|metadata| (name.clone(), metadata.allows_none))
            })
            .collect::<Vec<_>>();
        designation_metadata.extend(
            self.transaction
                .as_ref()
                .expect("transaction should exist")
                .created_states
                .iter()
                .filter(|(name, _)| !designation_deleted_states.contains(*name))
                .filter_map(|(name, cell)| {
                    cell.designation
                        .as_ref()
                        .map(|metadata| (name.clone(), metadata.allows_none))
                }),
        );
''',
        "lifetime cleanup designation discovery",
    ),
]

for old, new, label in replacements:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    text = text.replace(old, new, 1)

path.write_text(text)
