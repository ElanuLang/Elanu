from pathlib import Path

path = Path("compiler/src/runtime/persistence/partial.rs")
text = path.read_text()
variants = [
'''            Some(other) => {
                return Err(RuntimeError::new(format!(
                    "internal live designation '{designation}' carried {}, expected String identity",
                    other.type_name()
                )))
            }
''',
'''            Some(other) => return Err(RuntimeError::new(format!(
                "internal live designation '{designation}' carried {}, expected String identity",
                other.type_name()
            ))),
'''
]
replacement = '''            Some(other) => {
                let type_name = other.type_name();
                return Err(RuntimeError::new(format!(
                    "internal live designation '{designation}' carried {type_name}, expected String identity"
                )));
            }
'''
for variant in variants:
    if variant in text:
        path.write_text(text.replace(variant, replacement, 1))
        break
else:
    if replacement not in text:
        raise SystemExit("selected designation formatting variant not found")
