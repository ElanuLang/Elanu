from pathlib import Path

path = Path("compiler/src/runtime/persistence.rs")
text = path.read_text()
needle = "#[cfg(test)]\nmod partial_materialization_experiment;\n"
insert = "#[cfg(test)]\nmod opaque_dormant_backing_experiment;\n\n"
if insert not in text:
    if needle not in text:
        raise SystemExit("partial materialization module anchor not found")
    text = text.replace(needle, insert + needle, 1)
    path.write_text(text)
