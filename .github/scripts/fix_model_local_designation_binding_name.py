from pathlib import Path

path = Path("compiler/src/designation_runtime_metadata.rs")
text = path.read_text()
old = '''            designations.insert(
                format!("__meld_sm${}${}", root.name, format!("${}", member.name)),
                metadata.clone(),
            );
'''
new = '''            designations.insert(
                format!("__meld_sm${}${}", root.name, member.name),
                metadata.clone(),
            );
'''
count = text.count(old)
if count != 1:
    raise SystemExit(f"expected one model-member designation binding bridge, found {count}")
path.write_text(text.replace(old, new, 1))
