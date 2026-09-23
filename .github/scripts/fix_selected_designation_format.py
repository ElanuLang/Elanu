from pathlib import Path

path = Path("compiler/src/runtime/persistence/partial.rs")
text = path.read_text()
old = '''            Some(other) => return Err(RuntimeError::new(format!(
                "internal live designation '{designation}' carried {}, expected String identity",
                other.type_name()
            ))),
'''
new = '''            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal live designation '{designation}' carried {}, expected String identity",
                    other.type_name()
                )))
            }
'''
if old in text:
    path.write_text(text.replace(old, new, 1))
elif new not in text:
    raise SystemExit("format normalization anchor not found")
