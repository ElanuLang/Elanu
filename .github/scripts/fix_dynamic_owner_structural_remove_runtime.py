from pathlib import Path

path = Path("compiler/src/runtime.rs")
text = path.read_text()
old = '''        let target = if let Some(binding) = self
            .action_frames
            .last()
            .and_then(|frame| frame.bindings.get(target))
        {
            match binding {
                ActionBinding::State(state_name) => state_name.clone(),
                ActionBinding::Value(_) => {
                    return Err(RuntimeError::new(
                        "internal structural removal target is not writable state",
                    ))
                }
            }
        } else {
            target.clone()
        };
'''
new = '''        let target = self.resolve_state_grant(target)?;
'''
if text.count(old) != 1:
    raise SystemExit(f"expected one structural-removal target resolver, found {text.count(old)}")
path.write_text(text.replace(old, new, 1))
