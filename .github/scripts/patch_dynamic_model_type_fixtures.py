from pathlib import Path

for path in [
    Path("compiler/src/runtime/structural_move_execution.rs"),
    Path("compiler/src/runtime/relative_navigation_execution.rs"),
]:
    text = path.read_text()
    old = "        dynamic_model_owners: HashMap::new(),\n        runtime_index_grant_carriers: HashMap::new(),\n"
    new = "        dynamic_model_owners: HashMap::new(),\n        dynamic_model_types: HashMap::new(),\n        runtime_index_grant_carriers: HashMap::new(),\n"
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one Runtime fixture anchor, found {count}")
    path.write_text(text.replace(old, new, 1))
