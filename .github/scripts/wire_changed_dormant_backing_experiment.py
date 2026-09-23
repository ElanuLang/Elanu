from pathlib import Path

persistence = Path("compiler/src/runtime/persistence.rs")
text = persistence.read_text()
needle = "#[cfg(test)]\nmod incremental_dormant_publication_experiment;\n"
insert = "#[cfg(test)]\nmod changed_dormant_backing_experiment;\n\n"
if insert not in text:
    if needle not in text:
        raise SystemExit("incremental dormant publication module anchor not found")
    text = text.replace(needle, insert + needle, 1)
    persistence.write_text(text)

experiment = Path("compiler/src/runtime/persistence/changed_dormant_backing_experiment.rs")
text = experiment.read_text()
old = "    let provider = app.into_provider();\n    let accepted_folder_backing = provider.backing[&folder_token].clone();\n"
new = "    let mut provider = app.into_provider();\n    provider.backing_loads.clear();\n    let accepted_folder_backing = provider.backing[&folder_token].clone();\n"
if old in text:
    text = text.replace(old, new, 1)
    experiment.write_text(text)
elif new not in text:
    raise SystemExit("restart instrumentation anchor not found")
