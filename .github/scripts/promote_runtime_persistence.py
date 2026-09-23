from pathlib import Path

runtime = Path("compiler/src/runtime.rs")
text = runtime.read_text()
anchor = "#[cfg(test)]\nmod persistence_identity_metadata;\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected runtime module anchor once, found {text.count(anchor)}")
text = text.replace(anchor, anchor + "pub mod persistence;\n", 1)
runtime.write_text(text)

persistence = Path("compiler/src/runtime/persistence.rs")
text = persistence.read_text()
old = 'state model Folder {\n    state name = ""\n}'
new = 'state model Folder {\n    state name = ""\n    state folders: [live Folder] = []\n}'
if text.count(old) != 1:
    raise SystemExit(f"expected test model anchor once, found {text.count(old)}")
persistence.write_text(text.replace(old, new, 1))
