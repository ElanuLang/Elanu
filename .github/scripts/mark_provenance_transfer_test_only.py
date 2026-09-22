from pathlib import Path

path = Path("compiler/src/runtime.rs")
text = path.read_text()
old = "    fn transfer_runtime_model_owner(\n"
new = "    #[cfg(test)]\n    fn transfer_runtime_model_owner(\n"
if text.count(old) != 1:
    raise SystemExit(f"expected one transfer method, found {text.count(old)}")
path.write_text(text.replace(old, new, 1))
