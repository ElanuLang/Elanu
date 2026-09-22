from pathlib import Path

path = Path("compiler/src/runtime.rs")
text = path.read_text()
old = "#[cfg(test)]\nmod lifetime_termination_execution;\n#[cfg(test)]\nmod provenance_transfer_execution;\n"
new = "#[cfg(test)]\nmod lifetime_termination_execution;\n#[cfg(test)]\nmod model_local_designation_experiment;\n#[cfg(test)]\nmod provenance_transfer_execution;\n"
if text.count(old) != 1:
    raise SystemExit(f"expected one runtime test-module anchor, found {text.count(old)}")
path.write_text(text.replace(old, new, 1))
