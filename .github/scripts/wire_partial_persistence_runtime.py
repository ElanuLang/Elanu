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

development = Path("docs/DEVELOPMENT.md")
text = development.read_text()
anchor = "Providers may store these bytes but must not interpret the private payload. The byte format remains\nruntime implementation compatibility rather than Elanu source semantics or a provider schema.\n"
addition = '''\n\nFor large partially resident worlds, `runtime::persistence::partial` provides a separate production\nhost boundary without changing the existing monolithic `PersistentRuntime`. `PartialPersistentRuntime`\nbacks each dynamic modeled identity with a runtime-owned opaque key when that identity first becomes\ndurable. After process restart, backed identities retain exact model/lifetime identity metadata while\ntheir member cells remain dormant until the host explicitly calls `materialize` with one opaque key.\n\n`PartialPersistenceProvider` stores an opaque manifest plus opaque key/payload pairs and exposes one\natomic `replace_candidate(manifest, changed_backing)` operation. Resident-only actions may publish a\nnew manifest without reading or rewriting dormant backing. If one explicitly materialized identity\nchanges, only that identity's opaque payload plus the candidate manifest need be replaced; unrelated\nbacking remains untouched. Provider acceptance still precedes authoritative runtime publication.\n\nBacking keys, residency, manifest layout, and materialization are runtime/host infrastructure rather\nthan Elanu source concepts. The boundary does not select automatic loading, eviction/cache policy,\nbacking garbage collection, content addressing, a database/ORM/query API, or a concrete storage\nbackend. Providers must treat manifest, keys, and backing payloads as uninterpreted runtime-owned\nbytes and must not reconstruct model-member or generated-name semantics from them.\n'''
if addition.strip() not in text:
    if anchor not in text:
        raise SystemExit("DEVELOPMENT persistence anchor not found")
    development.write_text(text.replace(anchor, anchor + addition, 1))

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
bullet = "- Added production partial-residency persistence with opaque per-identity backing, explicit runtime materialization, and atomic candidate publication that rewrites only changed backing while preserving existing monolithic `PersistentRuntime` behavior.\n"
anchor = "### Compiler/runtime\n\n"
if bullet not in text:
    if anchor not in text:
        raise SystemExit("CHANGELOG compiler/runtime anchor not found")
    changelog.write_text(text.replace(anchor, anchor + bullet, 1))
