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
