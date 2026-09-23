from pathlib import Path

path = Path("compiler/src/runtime/restart_checkpoint_experiment.rs")
text = path.read_text()
anchor = "\n#[test]\nfn checkpoint_rejects_active_transaction_and_does_not_capture_staged_work() {\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected one insertion anchor, found {text.count(anchor)}")

addition = r'''
trait RuntimePersistenceProvider {
    fn load(&mut self) -> Result<Option<RuntimeCheckpoint>, RuntimeError>;
    fn replace(&mut self, checkpoint: &RuntimeCheckpoint) -> Result<(), RuntimeError>;
}

#[derive(Debug, Default)]
struct MemoryPersistenceProvider {
    current: Option<RuntimeCheckpoint>,
    reject_next_replace: bool,
    load_attempts: usize,
    replace_attempts: usize,
}

impl RuntimePersistenceProvider for MemoryPersistenceProvider {
    fn load(&mut self) -> Result<Option<RuntimeCheckpoint>, RuntimeError> {
        self.load_attempts += 1;
        Ok(self.current.clone())
    }

    fn replace(&mut self, checkpoint: &RuntimeCheckpoint) -> Result<(), RuntimeError> {
        self.replace_attempts += 1;
        if self.reject_next_replace {
            self.reject_next_replace = false;
            return Err(RuntimeError::new("persistence provider rejected replacement"));
        }
        self.current = Some(checkpoint.clone());
        Ok(())
    }
}

struct PersistentRuntime<P> {
    runtime: Runtime,
    checked: crate::CheckedSource,
    provider: P,
}

impl<P: RuntimePersistenceProvider> PersistentRuntime<P> {
    fn open(checked: crate::CheckedSource, mut provider: P) -> Result<Self, RuntimeError> {
        let mut runtime = Runtime::from_checked_source(&checked)?;
        if let Some(checkpoint) = provider.load()? {
            runtime.restore_restart_checkpoint(&checkpoint)?;
        }
        Ok(Self {
            runtime,
            checked,
            provider,
        })
    }

    fn run_action(&mut self, name: &str) -> Result<(), RuntimeError> {
        let provider = &mut self.provider;
        run_action_with_durable_acceptance(&mut self.runtime, &self.checked, name, |checkpoint| {
            provider.replace(checkpoint)
        })
    }
}

#[test]
fn empty_provider_opens_fresh_runtime_and_first_commit_becomes_durable() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut app = PersistentRuntime::open(checked, provider).expect("empty provider should open");

    assert_eq!(app.provider.load_attempts, 1);
    assert!(app.provider.current.is_none());

    app.run_action("seed").expect("first durable action should commit");
    assert_eq!(app.provider.replace_attempts, 1);
    assert_eq!(
        app.provider.current,
        Some(app.runtime.capture_restart_checkpoint().unwrap())
    );
}

#[test]
fn provider_load_reconstructs_the_same_committed_world() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut first = PersistentRuntime::open(checked.clone(), provider).expect("runtime should open");
    first.run_action("seed").expect("seed should commit durably");
    first
        .run_action("moveToTrash")
        .expect("Trash move should commit durably");
    let durable = first.provider.current.clone().expect("provider should hold image");

    let provider = first.provider;
    let mut restarted = PersistentRuntime::open(checked, provider).expect("restart should load image");
    assert_eq!(restarted.provider.load_attempts, 2);
    assert_eq!(restarted.runtime.capture_restart_checkpoint().unwrap(), durable);
    assert_eq!(restarted.runtime.value("selectedInTrash").unwrap(), Value::Bool(true));
    assert_eq!(restarted.runtime.value("selectedInSource").unwrap(), Value::Bool(false));

    restarted
        .run_action("restoreFromTrash")
        .expect("loaded restoreParent designation should drive durable restore");
    assert_eq!(
        restarted.provider.current,
        Some(restarted.runtime.capture_restart_checkpoint().unwrap())
    );
}

#[test]
fn provider_replace_failure_leaves_runtime_and_provider_on_same_prior_world() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut app = PersistentRuntime::open(checked, provider).expect("runtime should open");
    app.run_action("seed").expect("seed should commit durably");
    app.run_action("moveToTrash").expect("Trash move should commit durably");
    let prior = app.provider.current.clone().expect("prior durable image should exist");

    app.provider.reject_next_replace = true;
    app.run_action("restoreFromTrash")
        .expect_err("replace rejection must fail action");

    assert_eq!(app.provider.current, Some(prior.clone()));
    assert_eq!(app.runtime.capture_restart_checkpoint().unwrap(), prior);
    assert_eq!(app.runtime.value("selectedInTrash").unwrap(), Value::Bool(true));
}

#[test]
fn semantic_failure_does_not_call_provider_replace() {
    let checked = checked_source();
    let provider = MemoryPersistenceProvider::default();
    let mut app = PersistentRuntime::open(checked, provider).expect("runtime should open");
    app.run_action("seed").expect("seed should commit durably");
    let attempts = app.provider.replace_attempts;
    let prior = app.provider.current.clone().expect("durable image should exist");

    app.run_action("failedRename")
        .expect_err("semantic failure must fail before persistence replacement");

    assert_eq!(app.provider.replace_attempts, attempts);
    assert_eq!(app.provider.current, Some(prior.clone()));
    assert_eq!(app.runtime.capture_restart_checkpoint().unwrap(), prior);
}
'''

path.write_text(text.replace(anchor, "\n" + addition + anchor, 1))
