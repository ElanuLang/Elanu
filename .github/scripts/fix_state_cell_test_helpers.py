from pathlib import Path

for filename in [
    "compiler/src/runtime/structural_move_execution.rs",
    "compiler/src/runtime/relative_navigation_execution.rs",
]:
    path = Path(filename)
    text = path.read_text()
    old = '''    StateCell {
        value,
        value_type,
        dependents: HashSet::new(),
    }
'''
    new = '''    StateCell {
        value,
        value_type,
        designation: None,
        dependents: HashSet::new(),
    }
'''
    if text.count(old) != 1:
        raise SystemExit(f"{filename}: expected one StateCell helper, found {text.count(old)}")
    path.write_text(text.replace(old, new, 1))
