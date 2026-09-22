from pathlib import Path

path = Path('docs/LANGUAGE_REFERENCE.md')
text = path.read_text()
old_transfer = '# 21. Owner-relative child lifetime `transfer` — provisional/implemented\n'
old_reduce = '# 20. Read-only `reduce` — v0.8.0 provisional/implemented\n'
if text.count(old_transfer) != 1 or text.count(old_reduce) != 1:
    raise SystemExit('expected purge-adjacent heading numbering not found')
text = text.replace(old_transfer, '# 20. Owner-relative child lifetime `transfer` — provisional/implemented\n', 1)
text = text.replace(old_reduce, '# 21. Read-only `reduce` — v0.8.0 provisional/implemented\n', 1)
path.write_text(text)
