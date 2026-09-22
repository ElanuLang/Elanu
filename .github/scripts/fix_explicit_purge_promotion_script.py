from pathlib import Path

path = Path('.github/scripts/promote_explicit_subtree_purge.py')
text = path.read_text()
text = text.replace(
    "    ''') if name == DESTROY_BUILTIN_ACTION => {\\n''',\n    ''') if name == DESTROY_BUILTIN_ACTION || name == PURGE_BUILTIN_ACTION => {\\n''',",
    "    '''                } if name == DESTROY_BUILTIN_ACTION => {\\n''',\n    '''                } if name == DESTROY_BUILTIN_ACTION || name == PURGE_BUILTIN_ACTION => {\\n''',",
    1,
)
text = text.replace(
    "for old_num, new_num in [(20,21),(21,22),(22,23),(23,24),(24,25),(25,26),(26,27),(27,28),(28,29),(29,30),(30,31)]:",
    "for old_num, new_num in [(30,31),(29,30),(28,29),(27,28),(26,27),(25,26),(24,25),(23,24),(22,23),(21,22),(20,21)]:",
    1,
)
path.write_text(text)
