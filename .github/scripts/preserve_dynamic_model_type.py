from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))


path = Path("compiler/src/runtime.rs")

replace_once(
    path,
    "#[cfg(test)]\nmod model_local_designation_experiment;\n",
    "#[cfg(test)]\nmod model_local_designation_experiment;\n#[cfg(test)]\nmod persistence_identity_metadata;\n",
    "test module registration",
)

replace_once(
    path,
    "    created_model_owners: HashMap<String, String>,\n    updated_model_owners: HashMap<String, String>,\n",
    "    created_model_owners: HashMap<String, String>,\n    created_model_types: HashMap<String, String>,\n    updated_model_owners: HashMap<String, String>,\n",
    "transaction model type field",
)

replace_once(
    path,
    "    dynamic_model_owners: HashMap<String, String>,\n    runtime_index_grant_carriers: HashMap<String, Expr>,\n",
    "    dynamic_model_owners: HashMap<String, String>,\n    dynamic_model_types: HashMap<String, String>,\n    runtime_index_grant_carriers: HashMap<String, Expr>,\n",
    "runtime model type field",
)

replace_once(
    path,
    "            dynamic_model_owners: HashMap::new(),\n            runtime_index_grant_carriers: HashMap::new(),\n",
    "            dynamic_model_owners: HashMap::new(),\n            dynamic_model_types: HashMap::new(),\n            runtime_index_grant_carriers: HashMap::new(),\n",
    "runtime model type initialization",
)

replace_once(
    path,
    "        self.transaction\n            .as_mut()\n            .expect(\"transaction should exist while creating model\")\n            .created_model_owners\n            .insert(identity.clone(), owner_root.to_string());\n\n        let context = ModelRuntimeContext {\n",
    "        self.transaction\n            .as_mut()\n            .expect(\"transaction should exist while creating model\")\n            .created_model_owners\n            .insert(identity.clone(), owner_root.to_string());\n        self.transaction\n            .as_mut()\n            .expect(\"transaction should exist while creating model\")\n            .created_model_types\n            .insert(identity.clone(), model_name.to_string());\n\n        let context = ModelRuntimeContext {\n",
    "transactional model type creation",
)

replace_once(
    path,
    "            created_model_owners,\n            updated_model_owners,\n",
    "            created_model_owners,\n            created_model_types,\n            updated_model_owners,\n",
    "commit destructure model types",
)

replace_once(
    path,
    "        self.dynamic_model_owners.extend(created_model_owners);\n        self.dynamic_model_owners.extend(updated_model_owners);\n",
    "        self.dynamic_model_owners.extend(created_model_owners);\n        self.dynamic_model_types.extend(created_model_types);\n        self.dynamic_model_owners.extend(updated_model_owners);\n",
    "commit model types",
)

replace_once(
    path,
    "        for identity in terminated_model_identities {\n            self.dynamic_model_owners.remove(&identity);\n        }\n",
    "        for identity in terminated_model_identities {\n            self.dynamic_model_owners.remove(&identity);\n            self.dynamic_model_types.remove(&identity);\n        }\n",
    "remove terminated model type",
)
