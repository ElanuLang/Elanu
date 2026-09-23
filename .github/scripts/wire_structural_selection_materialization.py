from pathlib import Path

partial = Path("compiler/src/runtime/persistence/partial.rs")
text = partial.read_text()
anchor = '''    /// Explicitly materialize one exact backed identity identified only by its
    /// opaque runtime key. Repeated materialization of an already resident
    /// identity is a no-op.
    pub fn materialize(&mut self, key: &[u8]) -> Result<(), RuntimeError> {
'''
method = '''    /// Materialize the exact child identity at one current occurrence of a
    /// stored sequence owned by a statically declared modeled root. The host
    /// supplies only application-facing root/member/index selection facts;
    /// identity and backing-key correlation remain runtime-private.
    pub fn materialize_root_member_index(
        &mut self,
        root: &str,
        member: &str,
        index: usize,
    ) -> Result<(), RuntimeError> {
        let root_metadata = self
            .checked
            .runtime_model_roots
            .get(root)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("unknown modeled root '{root}'")))?;
        let template = self
            .checked
            .runtime_model_templates
            .get(&root_metadata.model_name)
            .ok_or_else(|| RuntimeError::new(format!(
                "modeled root '{root}' references unknown model '{}'",
                root_metadata.model_name
            )))?;
        let member_metadata = template.member(member).ok_or_else(|| {
            RuntimeError::new(format!(
                "state model '{}' has no member '{member}'",
                template.name
            ))
        })?;
        if member_metadata.kind != crate::runtime_model_templates::RuntimeModelMemberKind::State {
            return Err(RuntimeError::new(format!(
                "modeled root selection '{root}.{member}' must name stored sequence state"
            )));
        }
        let expected_model = match &member_metadata.value_type {
            ValueType::SequenceLive(model) => model.clone(),
            other => {
                return Err(RuntimeError::new(format!(
                    "modeled root selection '{root}.{member}' must be [live T], got {}",
                    show_type(other)
                )))
            }
        };
        let state_name = super::super::model_binding_name(root, member);
        let current = self
            .runtime
            .states
            .get(&state_name)
            .map(|cell| cell.value.clone())
            .ok_or_else(|| RuntimeError::new(format!(
                "runtime state for modeled root selection '{root}.{member}' is missing"
            )))?;
        let Value::Sequence {
            element_model,
            targets,
        } = current
        else {
            return Err(RuntimeError::new(format!(
                "runtime state for modeled root selection '{root}.{member}' is not a sequence"
            )));
        };
        if element_model != expected_model {
            return Err(RuntimeError::new(format!(
                "modeled root selection '{root}.{member}' contains live {element_model} but expects live {expected_model}"
            )));
        }
        let identity = targets.get(index).cloned().ok_or_else(|| {
            RuntimeError::new(format!(
                "modeled root selection index {index} is out of bounds for '{root}.{member}' of length {}",
                targets.len()
            ))
        })?;
        let handle = self.backed.get(&identity).ok_or_else(|| {
            RuntimeError::new(format!(
                "selected identity from '{root}.{member}[{index}]' has no partial-persistence backing"
            ))
        })?;
        if handle.model_name != expected_model {
            return Err(RuntimeError::new(format!(
                "selected identity from '{root}.{member}[{index}]' has model '{}' but sequence expects '{expected_model}'",
                handle.model_name
            )));
        }
        let key = encode_key(handle.token);
        self.materialize(&key)
    }

'''
if "pub fn materialize_root_member_index" not in text:
    if anchor not in text:
        raise SystemExit("partial materialize anchor not found")
    text = text.replace(anchor, method + anchor, 1)

# Add one action that creates a duplicate structural occurrence without changing child identity.
source_anchor = '''action auditOnly {
    audit = "audited"
}
'''
source_addition = source_anchor + '''
action duplicateColdOccurrence {
    insert coldFolder into workspace.folders
}
'''
if "action duplicateColdOccurrence" not in text:
    if source_anchor not in text:
        raise SystemExit("partial test source action anchor not found")
    text = text.replace(source_anchor, source_addition, 1)

# Add focused structural-selection tests before designation-selection tests.
test_anchor = '''    #[test]
    fn source_designation_materializes_exact_selected_dormant_identity() {
'''
tests = '''    #[test]
    fn structural_index_materializes_exact_selected_dormant_identity() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (cold_key, document_key) = keys_by_model(&runtime);

        runtime
            .materialize_root_member_index("workspace", "folders", 1)
            .unwrap();
        assert_eq!(runtime.provider.loads, vec![cold_key]);
        assert!(!runtime.provider.loads.contains(&document_key));
        assert_eq!(
            runtime.value("coldName").unwrap(),
            Value::String("Cold".into())
        );
    }

    #[test]
    fn invalid_structural_index_fails_without_backing_read() {
        let (checked, mut provider) = seeded_provider();
        provider.loads.clear();
        let mut runtime = PartialPersistentRuntime::open(checked, provider).unwrap();

        runtime
            .materialize_root_member_index("workspace", "folders", 99)
            .expect_err("out-of-range structural selection should fail");
        assert!(runtime.provider.loads.is_empty());
    }

    #[test]
    fn duplicate_occurrences_materialize_one_child_identity() {
        let (checked, provider) = seeded_provider();
        let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider).unwrap();
        runtime
            .materialize_designation("coldFolder")
            .expect("Cold Folder should materialize before duplicate insertion");
        runtime
            .run_action("duplicateColdOccurrence")
            .expect("duplicate structural occurrence should publish");
        let mut provider = runtime.into_provider();
        provider.loads.clear();
        let mut restarted = PartialPersistentRuntime::open(checked, provider).unwrap();
        let (cold_key, _) = keys_by_model(&restarted);

        restarted
            .materialize_root_member_index("workspace", "folders", 2)
            .unwrap();
        assert_eq!(restarted.provider.loads, vec![cold_key.clone()]);
        restarted
            .materialize_root_member_index("workspace", "folders", 1)
            .unwrap();
        assert_eq!(restarted.provider.loads, vec![cold_key]);
    }

'''
if "structural_index_materializes_exact_selected_dormant_identity" not in text:
    if test_anchor not in text:
        raise SystemExit("structural materialization test anchor not found")
    text = text.replace(test_anchor, tests + test_anchor, 1)
partial.write_text(text)

# Runtime/host documentation only.
dev = Path("docs/DEVELOPMENT.md")
text = dev.read_text()
anchor = '''`PartialPersistentRuntime::materialize_designation` lets host integration request materialization
through a top-level source `live T` / `maybe live T` designation name. `CheckedSource` preserves the
source-designation-to-lowered-binding relationship structurally, so the runtime resolves the exact
current target and its private backing handle without parsing generated names or exposing dynamic
identity/backing keys to the host. This remains explicit host-driven materialization, not automatic
loading on member access.
'''
addition = anchor + '''
`PartialPersistentRuntime::materialize_root_member_index` covers the bounded static-root structural
navigation case: the host supplies a source modeled-root name, stored `[live T]` member name, and
current numeric occurrence index. The runtime resolves the current exact child identity and private
backing handle internally. The index remains a transient structural occurrence selector rather than
persistent child identity, including when duplicate occurrences target the same modeled child.
'''
if "materialize_root_member_index" not in text:
    if anchor not in text:
        raise SystemExit("DEVELOPMENT designation materialization anchor not found")
    dev.write_text(text.replace(anchor, addition, 1))

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
bullet = "- Added host-driven partial-persistence materialization from a static modeled-root stored-sequence index, resolving exact child identity/backing inside the runtime while keeping numeric occurrence distinct from child identity.\n"
anchor = "### Compiler/runtime\n\n"
if bullet not in text:
    if anchor not in text:
        raise SystemExit("CHANGELOG compiler/runtime anchor not found")
    changelog.write_text(text.replace(anchor, anchor + bullet, 1))
