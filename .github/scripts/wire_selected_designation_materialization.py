from pathlib import Path

# Preserve source-designation -> lowered runtime binding structurally.
meta = Path("compiler/src/designation_runtime_metadata.rs")
text = meta.read_text()
anchor = "pub(crate) fn collect(program: &Program) -> HashMap<String, RuntimeDesignationMetadata> {"
addition = '''pub(crate) fn collect_bindings(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            let type_name = state.type_name.as_deref()?;
            if decode_live_type_name(type_name).is_none()
                && decode_maybe_live_type_name(type_name).is_none()
            {
                return None;
            }
            Some((
                state.name.clone(),
                format!("{LOWERED_DESIGNATION_PREFIX}{}", state.name),
            ))
        })
        .collect()
}

'''
if "fn collect_bindings" not in text:
    if anchor not in text:
        raise SystemExit("designation metadata anchor not found")
    meta.write_text(text.replace(anchor, addition + anchor, 1))

lib = Path("compiler/src/lib.rs")
text = lib.read_text()
field_anchor = "    pub(crate) runtime_designations: HashMap<String, RuntimeDesignationMetadata>,\n"
field = field_anchor + "    pub(crate) runtime_designation_bindings: HashMap<String, String>,\n"
if "runtime_designation_bindings" not in text:
    if field_anchor not in text:
        raise SystemExit("CheckedSource designation field anchor not found")
    text = text.replace(field_anchor, field, 1)

collect_anchor = "    let mut runtime_designations =\n        designation_runtime_metadata::collect(&runtime_reduction_lowered);\n"
collect = collect_anchor + "    let runtime_designation_bindings =\n        designation_runtime_metadata::collect_bindings(&runtime_reduction_lowered);\n"
if "collect_bindings(&runtime_reduction_lowered)" not in text:
    if collect_anchor not in text:
        raise SystemExit("runtime designation collection anchor not found")
    text = text.replace(collect_anchor, collect, 1)

init_anchor = "        runtime_designations,\n    })\n"
init = "        runtime_designations,\n        runtime_designation_bindings,\n    })\n"
if "        runtime_designation_bindings,\n" not in text:
    if init_anchor not in text:
        raise SystemExit("CheckedSource initializer anchor not found")
    text = text.replace(init_anchor, init, 1)
lib.write_text(text)

partial = Path("compiler/src/runtime/persistence/partial.rs")
text = partial.read_text()
method_anchor = '''    /// Explicitly materialize one exact backed identity identified only by its
    /// opaque runtime key. Repeated materialization of an already resident
    /// identity is a no-op.
    pub fn materialize(&mut self, key: &[u8]) -> Result<(), RuntimeError> {
'''
method = '''    /// Materialize the exact modeled identity currently carried by one
    /// top-level source designation. The host names application state, while
    /// dynamic identity and backing-key correlation remain runtime-private.
    pub fn materialize_designation(&mut self, designation: &str) -> Result<(), RuntimeError> {
        let lowered = self
            .checked
            .runtime_designation_bindings
            .get(designation)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!(
                "unknown top-level live designation '{designation}'"
            )))?;
        let identity = match self.runtime.states.get(&lowered).map(|cell| cell.value.clone()) {
            Some(Value::String(identity)) if !identity.is_empty() => identity,
            Some(Value::String(_)) => {
                return Err(RuntimeError::new(format!(
                    "live designation '{designation}' has no target"
                )))
            }
            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal live designation '{designation}' carried {}, expected String identity",
                    other.type_name()
                )))
            }
            None => {
                return Err(RuntimeError::new(format!(
                    "runtime binding for live designation '{designation}' is missing"
                )))
            }
        };
        let handle = self.backed.get(&identity).ok_or_else(|| {
            RuntimeError::new(format!(
                "live designation '{designation}' targets an identity without partial-persistence backing"
            ))
        })?;
        let key = encode_key(handle.token);
        self.materialize(&key)
    }

'''
if "pub fn materialize_designation" not in text:
    if method_anchor not in text:
        raise SystemExit("partial materialize anchor not found")
    text = text.replace(method_anchor, method + method_anchor, 1)

# Add two focused tests before the existing changed-identity test.
test_anchor = "    #[test]\n    fn one_materialized_identity_changes_without_touching_unrelated_backing() {\n"
tests = '''    #[test]
    fn source_designation_materializes_exact_selected_dormant_identity() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (folder_key, document_key) = keys_by_model(&runtime);

        runtime.materialize_designation("coldFolder").unwrap();
        assert_eq!(runtime.provider.loads, vec![folder_key]);
        assert!(!runtime.provider.loads.contains(&document_key));
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold".into())
        );
    }

    #[test]
    fn designation_materialization_failure_does_not_partially_install_members() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let (folder_key, _) = {
            let runtime = PartialPersistentRuntime::open(checked.clone(), provider.clone()).unwrap();
            keys_by_model(&runtime)
        };
        provider.backing.remove(&folder_key);
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();

        runtime
            .materialize_designation("coldFolder")
            .expect_err("missing backing should reject selected materialization");
        assert!(runtime.value("coldName").is_err());
        assert_eq!(runtime.provider.loads, vec![folder_key]);
    }

'''
if "source_designation_materializes_exact_selected_dormant_identity" not in text:
    if test_anchor not in text:
        raise SystemExit("partial persistence test anchor not found")
    text = text.replace(test_anchor, tests + test_anchor, 1)
partial.write_text(text)

# Runtime/host docs only: no language reference change.
dev = Path("docs/DEVELOPMENT.md")
text = dev.read_text()
anchor = "Backing keys, residency, manifest layout, and materialization are runtime/host infrastructure rather\n"
addition = '''`PartialPersistentRuntime::materialize_designation` lets host integration request materialization\nthrough a top-level source `live T` / `maybe live T` designation name. `CheckedSource` preserves the\nsource-designation-to-lowered-binding relationship structurally, so the runtime resolves the exact\ncurrent target and its private backing handle without parsing generated names or exposing dynamic\nidentity/backing keys to the host. This remains explicit host-driven materialization, not automatic\nloading on member access.\n\n'''
if "materialize_designation" not in text:
    if anchor not in text:
        raise SystemExit("DEVELOPMENT partial persistence anchor not found")
    text = text.replace(anchor, addition + anchor, 1)
    dev.write_text(text)

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
bullet = "- Added host-driven partial-persistence materialization by top-level source live designation, preserving the source-to-lowered runtime binding structurally so hosts need not see dynamic identities or backing keys.\n"
anchor = "### Compiler/runtime\n\n"
if bullet not in text:
    if anchor not in text:
        raise SystemExit("CHANGELOG compiler/runtime anchor not found")
    changelog.write_text(text.replace(anchor, anchor + bullet, 1))
