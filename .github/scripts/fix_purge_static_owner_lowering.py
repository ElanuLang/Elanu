from pathlib import Path

path = Path('compiler/src/lifetime_termination_surface.rs')
text = path.read_text()
old = '''                } if name == DESTROY_BUILTIN_ACTION => {
'''
new = '''                } if name == DESTROY_BUILTIN_ACTION || name == PURGE_BUILTIN_ACTION => {
'''
count = text.count(old)
if count != 1:
    raise SystemExit(f'expected one remaining destroy-only owner-lowering match, found {count}')
path.write_text(text.replace(old, new, 1))
