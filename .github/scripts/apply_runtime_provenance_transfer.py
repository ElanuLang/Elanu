from pathlib import Path

path = Path("compiler/src/runtime.rs")
text = path.read_text()

def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    text = text.replace(old, new, 1)

replace_once(
'''#[cfg(test)]
mod lifetime_termination_execution;
#[cfg(test)]
mod structural_move_execution;
''',
'''#[cfg(test)]
mod lifetime_termination_execution;
#[cfg(test)]
mod provenance_transfer_execution;
#[cfg(test)]
mod structural_move_execution;
''',
"test module registration",
)

replace_once(
'''    created_derived: HashMap<String, DerivedCell>,
    created_model_owners: HashMap<String, String>,
    deleted_states: HashSet<String>,
''',
'''    created_derived: HashMap<String, DerivedCell>,
    created_model_owners: HashMap<String, String>,
    updated_model_owners: HashMap<String, String>,
    deleted_states: HashSet<String>,
''',
"transaction owner overlay",
)

old = '''    fn model_identity_exists(&self, root: &str) -> bool {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.terminated_model_identities.contains(root))
        {
            return false;
        }

        self.runtime_model_roots.contains_key(root)
            || self.dynamic_model_owners.contains_key(root)
            || self
                .transaction
                .as_ref()
                .is_some_and(|transaction| transaction.created_model_owners.contains_key(root))
    }

'''
new = '''    fn model_identity_exists(&self, root: &str) -> bool {
        if self
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.terminated_model_identities.contains(root))
        {
            return false;
        }

        self.runtime_model_roots.contains_key(root)
            || self.dynamic_model_owners.contains_key(root)
            || self
                .transaction
                .as_ref()
                .is_some_and(|transaction| transaction.created_model_owners.contains_key(root))
    }

    fn current_dynamic_model_owner(&self, identity: &str) -> Option<String> {
        let transaction = self.transaction.as_ref();
        if transaction.is_some_and(|transaction| {
            transaction.terminated_model_identities.contains(identity)
        }) {
            return None;
        }

        transaction
            .and_then(|transaction| transaction.updated_model_owners.get(identity))
            .or_else(|| {
                transaction.and_then(|transaction| transaction.created_model_owners.get(identity))
            })
            .or_else(|| self.dynamic_model_owners.get(identity))
            .cloned()
    }

    fn has_live_dynamic_child_rooted_in(&self, owner: &str) -> bool {
        let committed_child = self.dynamic_model_owners.keys().any(|child| {
            self.current_dynamic_model_owner(child).as_deref() == Some(owner)
        });
        if committed_child {
            return true;
        }

        self.transaction.as_ref().is_some_and(|transaction| {
            transaction
                .created_model_owners
                .iter()
                .any(|(child, child_owner)| {
                    !transaction.terminated_model_identities.contains(child)
                        && child_owner == owner
                })
        })
    }

    fn transfer_runtime_model_owner(
        &mut self,
        identity: &str,
        expected_owner: &str,
        destination_owner: &str,
    ) -> Result<(), RuntimeError> {
        let Some(transaction) = self.transaction.as_ref() else {
            return Err(RuntimeError::new(
                "runtime model provenance can only transfer inside an active action transaction",
            ));
        };

        if transaction.terminated_model_identities.contains(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child is already terminated in this transaction",
            ));
        }
        if !self.dynamic_model_owners.contains_key(identity) {
            return Err(RuntimeError::new(
                "runtime provenance transfer requires an existing committed dynamic child",
            ));
        }

        let actual_owner = self
            .current_dynamic_model_owner(identity)
            .expect("committed live dynamic child should have an owner");
        if actual_owner != expected_owner {
            return Err(RuntimeError::new(format!(
                "runtime modeled-state child is currently rooted in '{actual_owner}', not expected owner '{expected_owner}'"
            )));
        }

        if identity == destination_owner {
            return Err(RuntimeError::new(
                "runtime modeled-state child cannot become its own rooting owner",
            ));
        }
        if !self.model_identity_exists(destination_owner) {
            return Err(RuntimeError::new(format!(
                "unknown modeled-state destination owner '{destination_owner}'"
            )));
        }
        if self.has_live_dynamic_child_rooted_in(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child roots another live child; subtree provenance transfer is not selected",
            ));
        }

        if actual_owner == destination_owner {
            return Ok(());
        }

        self.transaction
            .as_mut()
            .expect("transaction should exist while transferring model provenance")
            .updated_model_owners
            .insert(identity.to_string(), destination_owner.to_string());
        Ok(())
    }

'''
replace_once(old, new, "runtime owner helpers")

replace_once(
'''        let Some(actual_owner) = self.dynamic_model_owners.get(identity).cloned() else {
            return Err(RuntimeError::new(
                "runtime lifetime termination requires an existing committed dynamic child",
            ));
        };
''',
'''        if !self.dynamic_model_owners.contains_key(identity) {
            return Err(RuntimeError::new(
                "runtime lifetime termination requires an existing committed dynamic child",
            ));
        }
        let actual_owner = self
            .current_dynamic_model_owner(identity)
            .expect("committed live dynamic child should have an owner");
''',
"transaction-visible termination owner",
)

replace_once(
'''        let has_committed_descendant = self.dynamic_model_owners.iter().any(|(child, owner)| {
            owner == identity && !transaction.terminated_model_identities.contains(child)
        });
        let has_created_descendant = transaction
            .created_model_owners
            .values()
            .any(|owner| owner == identity);
        if has_committed_descendant || has_created_descendant {
''',
'''        if self.has_live_dynamic_child_rooted_in(identity) {
''',
"transaction-visible descendant check",
)

replace_once(
'''            created_derived,
            created_model_owners,
            deleted_states,
''',
'''            created_derived,
            created_model_owners,
            updated_model_owners,
            deleted_states,
''',
"commit destructuring",
)

replace_once(
'''        self.states.extend(created_states);
        self.derived.extend(created_derived);
        self.dynamic_model_owners.extend(created_model_owners);

        let mut changed = Vec::new();
''',
'''        self.states.extend(created_states);
        self.derived.extend(created_derived);
        self.dynamic_model_owners.extend(created_model_owners);
        self.dynamic_model_owners.extend(updated_model_owners);

        let mut changed = Vec::new();
''',
"commit owner replacement",
)

path.write_text(text)
