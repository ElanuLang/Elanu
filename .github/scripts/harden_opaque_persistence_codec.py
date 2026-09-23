from pathlib import Path

path = Path("compiler/src/runtime/persistence.rs")
text = path.read_text()
replacements = {
    "let mut static_states = Vec::with_capacity(static_count);": "let mut static_states = Vec::new();",
    "let mut model_states = Vec::with_capacity(model_count);": "let mut model_states = Vec::new();",
    "let mut roots = Vec::with_capacity(root_count);": "let mut roots = Vec::new();",
    "let mut targets = Vec::with_capacity(count);": "let mut targets = Vec::new();",
    "let mut values = HashMap::with_capacity(count);": "let mut values = HashMap::new();",
    "let mut models = HashMap::with_capacity(count);": "let mut models = HashMap::new();",
}
for old, new in replacements.items():
    if text.count(old) != 1:
        raise SystemExit(f"expected exactly one occurrence of {old!r}, found {text.count(old)}")
    text = text.replace(old, new, 1)

anchor = '''    #[test]\n    fn malformed_bytes_fail_during_decode_before_runtime_restore() {\n'''
if text.count(anchor) != 1:
    raise SystemExit(f"expected malformed test anchor once, found {text.count(anchor)}")
addition = r'''    #[test]
    fn forged_huge_collection_count_fails_without_preallocating_that_count() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PERSISTENCE_ENCODING_MAGIC);
        bytes.extend_from_slice(&PERSISTENCE_ENCODING_VERSION.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());

        let error = PersistenceImage::decode(&bytes)
            .expect_err("forged huge collection count must fail as truncated data");
        assert!(error.message.contains("truncated"));
    }

'''
text = text.replace(anchor, addition + anchor, 1)
path.write_text(text)
