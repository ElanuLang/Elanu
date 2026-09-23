from pathlib import Path

runtime = Path("compiler/src/runtime.rs")
text = runtime.read_text()
old = "impl RuntimeError {\n    fn new(message: impl Into<String>) -> Self {"
new = "impl RuntimeError {\n    pub fn new(message: impl Into<String>) -> Self {"
if text.count(old) != 1:
    raise SystemExit(f"expected RuntimeError constructor once, found {text.count(old)}")
runtime.write_text(text.replace(old, new, 1))

dev = Path("docs/DEVELOPMENT.md")
text = dev.read_text()
anchor = "## Rust validation\n"
section = '''## Runtime persistence host boundary

The public runtime exposes a provider-agnostic persistence boundary in `runtime::persistence`.
This is implementation infrastructure, not Elanu source syntax.

`PersistenceProvider` has two synchronous operations:

```text
load() -> zero or one opaque PersistenceImage
replace(candidate) -> accept the complete replacement or fail without changing the prior image
```

`PersistentRuntime` restores a loaded image only when its runtime-owned persisted reconstruction
shape is compatible with the current checked application. Top-level actions execute against the
ordinary Elanu transaction machinery, produce a candidate committed world, ask the provider to
accept the candidate image, and publish the candidate runtime only after acceptance. Semantic
action failure does not call the provider; provider failure leaves the prior runtime world in
place.

`PersistenceImage` is intentionally opaque to providers. Private generated runtime names may
occur inside the runtime-owned encoding, but providers must not parse them or treat them as host
semantics. Persistence compatibility is based on stored reconstruction topology rather than
source text or whole-program identity, so formatting and action-body-only changes do not by
themselves invalidate an image.

This boundary does not select a disk/database format, source `save`/`load`, schema migration,
async durability, crash recovery, retries, or general external-effect semantics.

'''
if text.count(anchor) != 1:
    raise SystemExit(f"expected DEVELOPMENT anchor once, found {text.count(anchor)}")
dev.write_text(text.replace(anchor, section + anchor, 1))

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
anchor = "### Compiler/runtime\n\n"
entry = "- Promoted the proven restart image, persisted-shape compatibility check, provider `load`/atomic-`replace` boundary, and candidate-before-publication ordering into the public synchronous `runtime::persistence` host API without adding source persistence syntax or choosing a storage backend.\n"
if text.count(anchor) < 1:
    raise SystemExit("CHANGELOG compiler/runtime anchor missing")
text = text.replace(anchor, anchor + entry, 1)
changelog.write_text(text)
