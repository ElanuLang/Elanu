from pathlib import Path

path = Path("compiler/src/runtime.rs")
text = path.read_text()
old = "#[cfg(test)]\nmod provenance_transfer_execution;\n"
new = "#[cfg(test)]\nmod provenance_transfer_execution;\n#[cfg(test)]\nmod restart_checkpoint_experiment;\n"
count = text.count(old)
if count != 1:
    raise SystemExit(f"expected one runtime test module anchor, found {count}")
path.write_text(text.replace(old, new, 1))
