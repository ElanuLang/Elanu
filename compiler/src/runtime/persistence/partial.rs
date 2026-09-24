include!("partial/core.rs");

impl<P: PartialPersistenceProvider> PartialPersistentRuntime<P> {
    /// Invoke a top-level action with ordinary host-supplied primitive values.
    ///
    /// Modeled identity sequences are deliberately excluded from this boundary;
    /// hosts must select/materialize identities through the runtime's explicit
    /// designation and structural APIs instead of reconstructing identity values.
    pub fn run_action_with_values(
        &mut self,
        name: &str,
        arguments: &[Value],
    ) -> Result<(), RuntimeError> {
        let arguments = arguments
            .iter()
            .map(host_value_action_argument)
            .collect::<Result<Vec<_>, _>>()?;

        let mut suffix = 0usize;
        let wrapper_name = loop {
            let candidate = format!("__elanu_host_invoke${suffix}");
            if candidate != name && !self.runtime.actions.contains_key(&candidate) {
                break candidate;
            }
            suffix = suffix
                .checked_add(1)
                .ok_or_else(|| RuntimeError::new("host action wrapper name space exhausted"))?;
        };

        self.runtime.actions.insert(
            wrapper_name.clone(),
            ActionDecl {
                location: crate::ast::SourceLocation::new(1, 1),
                name: wrapper_name.clone(),
                parameters: Vec::new(),
                statements: vec![Statement::ActionCall {
                    location: crate::ast::SourceLocation::new(1, 1),
                    name: name.to_string(),
                    arguments,
                }],
            },
        );
        self.runtime
            .action_parameter_types
            .insert(wrapper_name.clone(), Vec::new());

        let result = self.run_action(&wrapper_name);
        self.runtime.actions.remove(&wrapper_name);
        self.runtime.action_parameter_types.remove(&wrapper_name);
        result
    }
}

fn host_value_action_argument(value: &Value) -> Result<ActionArgument, RuntimeError> {
    let expression = match value {
        Value::Int(value) => Expr::Integer(*value),
        Value::Float(value) => Expr::Float(*value),
        Value::Bool(value) => Expr::Bool(*value),
        Value::String(value) => Expr::String(value.clone()),
        Value::Sequence { .. } => {
            return Err(RuntimeError::new(
                "partial persistent host action values cannot carry modeled identity sequences",
            ))
        }
    };
    Ok(ActionArgument::Value(expression))
}
