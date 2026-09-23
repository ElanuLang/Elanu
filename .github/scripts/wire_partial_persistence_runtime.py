from pathlib import Path

path = Path("compiler/src/runtime/persistence.rs")
text = path.read_text()
needle = "use super::*;\n"
insert = "use super::*;\n\npub mod partial;\n"
if "pub mod partial;" not in text:
    if needle not in text:
        raise SystemExit("persistence module import anchor not found")
    text = text.replace(needle, insert, 1)
    path.write_text(text)

partial = Path("compiler/src/runtime/persistence/partial.rs")
text = partial.read_text()
start = text.find("    fn keys_by_model(runtime: &PartialPersistentRuntime<MemoryProvider>)")
end = text.find("\n    #[test]", start)
if start == -1 or end == -1:
    raise SystemExit("backing-key test helper boundaries not found")
new = '''    fn keys_by_model(runtime: &PartialPersistentRuntime<MemoryProvider>) -> (Vec<u8>, Vec<u8>) {
        let folder = runtime
            .backed
            .iter()
            .find_map(|(identity, handle)| {
                if handle.model_name != "Folder" {
                    return None;
                }
                let key = encode_key(handle.token);
                let payload = runtime.provider.backing.get(&key)?;
                let values = decode_backing(&runtime.checked, identity, handle, payload).ok()?;
                matches!(values.get("name"), Some(Value::String(name)) if name == "Cold")
                    .then_some(key)
            })
            .expect("Cold Folder backing should exist");
        let document = runtime
            .backed
            .iter()
            .find_map(|(_, handle)| {
                (handle.model_name == "Document").then(|| encode_key(handle.token))
            })
            .expect("Document backing should exist");
        (folder, document)
    }
'''
text = text[:start] + new + text[end:]
partial.write_text(text)
