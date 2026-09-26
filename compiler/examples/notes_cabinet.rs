use std::{
    collections::HashMap,
    error::Error,
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    path::PathBuf,
};

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
    CheckedSource,
};

const SOURCE: &str = include_str!("../../examples/cabinet.elnu");
const SNAPSHOT_MAGIC: &[u8; 8] = b"ELANCAB1";
const SNAPSHOT_VERSION: u32 = 1;

type CabinetRuntime = PartialPersistentRuntime<DirectoryProvider>;
type AppResult<T> = Result<T, Box<dyn Error>>;

// Host boundary: this example may retain terminal input/status state, but it must not
// retain modeled identities, structural membership, application selection, parentage,
// trash state, restore semantics, or any other authoritative Cabinet application fact.
fn main() -> AppResult<()> {
    let checked = checked_source()?;
    let world_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".elanu-cabinet"));
    let provider = DirectoryProvider::open(world_dir)?;
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)?;

    if bool_value(&mut runtime, "initialized")? {
        materialize_visible(&mut runtime)?;
    } else {
        runtime.run_action("initialize")?;
    }

    let mut status = String::from("Notes Cabinet opened.");
    loop {
        render(&mut runtime, &status)?;
        let command = prompt("cabinet")?;
        status = match command.trim() {
            "q" | "quit" => break,
            "h" | "home" => run_visible(&mut runtime, "selectHome"),
            "ot" | "open-trash" => run_visible(&mut runtime, "openTrash"),
            "f" | "folder" => select_index(&mut runtime, "selectFolder", "child folder index"),
            "n" | "note" => select_index(&mut runtime, "selectNote", "note index"),
            "nf" | "new-folder" => create_folder(&mut runtime),
            "nn" | "new-note" => create_note(&mut runtime),
            "rf" | "rename-folder" => rename_folder(&mut runtime),
            "e" | "edit" => edit_note(&mut runtime),
            "t" | "trash" => run(&mut runtime, "trashSelectedNote"),
            "r" | "restore" => run_visible(&mut runtime, "restoreSelectedNote"),
            "d" | "delete" => run(&mut runtime, "permanentlyDeleteSelectedNote"),
            "x" | "fail" => demonstrate_rollback(&mut runtime),
            "restart" => match restart(runtime, checked.clone()) {
                Ok(restarted) => {
                    runtime = restarted;
                    String::from("Reopened the same persisted Elanu world.")
                }
                Err((prior, error)) => {
                    runtime = prior;
                    format!("Restart failed: {error}")
                }
            },
            "?" | "help" => String::from("Commands are shown below."),
            "" => String::new(),
            other => format!("Unknown command: {other}"),
        };
    }

    println!("Cabinet closed. Durable world remains in the selected world directory.");
    Ok(())
}

fn checked_source() -> AppResult<CheckedSource> {
    check_source_with_runtime_models(SOURCE).map_err(|diagnostics| {
        let message = diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        io::Error::other(message).into()
    })
}

fn materialize_visible(runtime: &mut CabinetRuntime) -> Result<(), RuntimeError> {
    runtime.materialize_designation("selectedFolder")?;
    if bool_value(runtime, "selectedNotePresent")
        .map_err(|error| RuntimeError::new(error.to_string()))?
    {
        runtime.materialize_designation("selectedNote")?;
        runtime.materialize_designation("trashFolder")?;
    }
    Ok(())
}

fn render(runtime: &mut CabinetRuntime, status: &str) -> AppResult<()> {
    let folder = string_value(runtime, "selectedFolderName")?;
    let note_present = bool_value(runtime, "selectedNotePresent")?;
    let note_title = string_value(runtime, "selectedNoteTitle")?;
    let note_body = string_value(runtime, "selectedNoteBody")?;
    let in_trash = if note_present {
        bool_value(runtime, "selectedNoteInTrash")?
    } else {
        false
    };

    let folders = designation_member_strings(runtime, "selectedFolder", "folders", "name")?;
    let notes = designation_member_strings(runtime, "selectedFolder", "notes", "title")?;
    let current_marker = if folder == "Trash" { "TRASH" } else { "FOLDER" };

    print!("\x1b[2J\x1b[H");
    println!("┌─ ELANU NOTES CABINET ─────────────────────────────────────────────────────┐");
    println!("│ {current_marker}: {folder}");
    println!("├─ CONTENTS ────────────────────────────────────────────────────────────────┤");
    println!("│ {folder}/");

    if folders.is_empty() && notes.is_empty() {
        println!("│   └─ <empty>");
    } else {
        let total = folders.len() + notes.len();
        let mut rendered = 0usize;

        for (index, name) in folders.iter().enumerate() {
            rendered += 1;
            let branch = if rendered == total {
                "└─"
            } else {
                "├─"
            };
            println!("│   {branch} [f{index}] {name}/");
        }
        for (index, title) in notes.iter().enumerate() {
            rendered += 1;
            let branch = if rendered == total {
                "└─"
            } else {
                "├─"
            };
            println!("│   {branch} [n{index}] {title}");
        }
    }

    println!("├─ SELECTED NOTE ──────────────────────────────────────────────────────────┤");
    if note_present {
        let badge = if in_trash { "  [TRASHED]" } else { "" };
        println!("│ {note_title}{badge}");
        println!("│");
        if note_body.is_empty() {
            println!("│ <empty body>");
        } else {
            for line in note_body.lines() {
                println!("│ {line}");
            }
        }
    } else {
        println!("│ <none selected>");
    }

    println!("├─ PLACES ─────────────────────────────────────────────────────────────────┤");
    println!("│ [h] Home / Notes                         [ot] Trash");
    println!("├─ COMMANDS ───────────────────────────────────────────────────────────────┤");
    println!("│ Open       [f] child folder by index   [n] note by index");
    println!("│ Create     [nf] folder                 [nn] note");
    println!("│ Edit       [rf] rename folder          [e] edit selected note");
    println!("│ Lifetime   [t] trash                   [r] restore   [d] permanent delete");
    println!("│ Test       [x] rollback proof          [restart] reopen persisted world");
    println!("│ Other      [?] help                    [q] quit");
    println!("├─ STATUS ─────────────────────────────────────────────────────────────────┤");
    if status.is_empty() {
        println!("│ Ready.");
    } else {
        println!("│ {status}");
    }
    println!("└──────────────────────────────────────────────────────────────────────────┘");
    println!();
    io::stdout().flush()?;
    Ok(())
}

fn prompt(label: &str) -> AppResult<String> {
    print!("{label}> ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_string())
}

fn run(runtime: &mut CabinetRuntime, action: &str) -> String {
    match runtime.run_action(action) {
        Ok(()) => format!("Committed {action}."),
        Err(error) => format!("{action} failed: {error}"),
    }
}

fn run_visible(runtime: &mut CabinetRuntime, action: &str) -> String {
    match runtime.run_action(action) {
        Ok(()) => match materialize_visible(runtime) {
            Ok(()) => format!("Committed {action}."),
            Err(error) => {
                format!("Action committed, but visible materialization failed: {error}")
            }
        },
        Err(error) => format!("{action} failed: {error}"),
    }
}

fn select_index(runtime: &mut CabinetRuntime, action: &str, label: &str) -> String {
    let value = match prompt(label) {
        Ok(value) => value,
        Err(error) => return format!("Input failed: {error}"),
    };
    let index = match value.parse::<i64>() {
        Ok(index) => index,
        Err(_) => return format!("'{value}' is not an integer index."),
    };
    match runtime.run_action_with_values(action, &[Value::Int(index)]) {
        Ok(()) => {
            if let Err(error) = materialize_visible(runtime) {
                format!("Selection committed, but visible materialization failed: {error}")
            } else {
                format!("Committed {action}({index}).")
            }
        }
        Err(error) => format!("{action} failed: {error}"),
    }
}

fn create_folder(runtime: &mut CabinetRuntime) -> String {
    let name = match prompt("folder name") {
        Ok(name) => name,
        Err(error) => return format!("Input failed: {error}"),
    };
    match runtime.run_action_with_values("createFolder", &[Value::String(name)]) {
        Ok(()) => String::from("Created and selected folder."),
        Err(error) => format!("createFolder failed: {error}"),
    }
}

fn create_note(runtime: &mut CabinetRuntime) -> String {
    let title = match prompt("title") {
        Ok(title) => title,
        Err(error) => return format!("Input failed: {error}"),
    };
    let body = match prompt("body") {
        Ok(body) => body,
        Err(error) => return format!("Input failed: {error}"),
    };
    match runtime.run_action_with_values("createNote", &[Value::String(title), Value::String(body)])
    {
        Ok(()) => String::from("Created and selected note."),
        Err(error) => format!("createNote failed: {error}"),
    }
}

fn rename_folder(runtime: &mut CabinetRuntime) -> String {
    let name = match prompt("new folder name") {
        Ok(name) => name,
        Err(error) => return format!("Input failed: {error}"),
    };
    match runtime.run_action_with_values("renameSelectedFolder", &[Value::String(name)]) {
        Ok(()) => String::from("Renamed selected folder."),
        Err(error) => format!("renameSelectedFolder failed: {error}"),
    }
}

fn edit_note(runtime: &mut CabinetRuntime) -> String {
    let title = match prompt("new title") {
        Ok(title) => title,
        Err(error) => return format!("Input failed: {error}"),
    };
    let body = match prompt("new body") {
        Ok(body) => body,
        Err(error) => return format!("Input failed: {error}"),
    };
    match runtime.run_action_with_values(
        "editSelectedNote",
        &[Value::String(title), Value::String(body)],
    ) {
        Ok(()) => String::from("Edited selected note."),
        Err(error) => format!("editSelectedNote failed: {error}"),
    }
}

fn demonstrate_rollback(runtime: &mut CabinetRuntime) -> String {
    let before = match string_value(runtime, "selectedNoteTitle") {
        Ok(value) => value,
        Err(error) => return format!("Could not read prior title: {error}"),
    };
    let attempted = match prompt("title to roll back") {
        Ok(value) => value,
        Err(error) => return format!("Input failed: {error}"),
    };
    match runtime.run_action_with_values("failEditSelectedNote", &[Value::String(attempted)]) {
        Ok(()) => String::from("Unexpectedly committed failing edit."),
        Err(error) => match string_value(runtime, "selectedNoteTitle") {
            Ok(after) if after == before => {
                format!("Action failed as requested ({error}); title remained '{after}'.")
            }
            Ok(after) => format!("Rollback proof failed: title changed to '{after}'."),
            Err(read_error) => format!("Action failed ({error}); reread failed: {read_error}"),
        },
    }
}

fn restart(
    runtime: CabinetRuntime,
    checked: CheckedSource,
) -> Result<CabinetRuntime, (CabinetRuntime, Box<dyn Error>)> {
    let provider = runtime.into_provider();
    let fallback_provider = provider.clone();
    match PartialPersistentRuntime::open(checked.clone(), provider) {
        Ok(mut restarted) => match materialize_visible(&mut restarted) {
            Ok(()) => Ok(restarted),
            Err(error) => {
                let provider = restarted.into_provider();
                let mut prior = PartialPersistentRuntime::open(checked, provider)
                    .expect("published cabinet world should reopen");
                let _ = materialize_visible(&mut prior);
                Err((prior, Box::new(error)))
            }
        },
        Err(error) => {
            let mut prior = PartialPersistentRuntime::open(checked, fallback_provider)
                .expect("published cabinet world should reopen");
            let _ = materialize_visible(&mut prior);
            Err((prior, Box::new(error)))
        }
    }
}

fn designation_member_strings(
    runtime: &mut CabinetRuntime,
    designation: &str,
    member: &str,
    child_member: &str,
) -> AppResult<Vec<String>> {
    let len = runtime.designation_member_len(designation, member)?;
    let mut values = Vec::with_capacity(len);

    for index in 0..len {
        match runtime.designation_member_index_value(designation, member, index, child_member)? {
            Value::String(value) => values.push(value),
            other => {
                return Err(io::Error::other(format!(
                    "'{designation}.{member}[{index}].{child_member}' produced {other}, expected String"
                ))
                .into());
            }
        }
    }

    Ok(values)
}

fn string_value(runtime: &mut CabinetRuntime, name: &str) -> AppResult<String> {
    match runtime.value(name)? {
        Value::String(value) => Ok(value),
        other => {
            Err(io::Error::other(format!("'{name}' produced {other}, expected String")).into())
        }
    }
}

fn bool_value(runtime: &mut CabinetRuntime, name: &str) -> AppResult<bool> {
    match runtime.value(name)? {
        Value::Bool(value) => Ok(value),
        other => Err(io::Error::other(format!("'{name}' produced {other}, expected Bool")).into()),
    }
}

#[derive(Debug, Clone)]
struct DirectoryProvider {
    directory: PathBuf,
    generation: u64,
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
}

impl DirectoryProvider {
    fn open(directory: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        let mut latest: Option<(u64, Vec<u8>, HashMap<Vec<u8>, Vec<u8>>)> = None;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("bin") {
                continue;
            }
            let bytes = fs::read(&path)?;
            let decoded = decode_snapshot(&bytes)?;
            if latest
                .as_ref()
                .is_none_or(|(generation, _, _)| decoded.0 > *generation)
            {
                latest = Some(decoded);
            }
        }

        let (generation, manifest, backing) = latest.unwrap_or_default();
        Ok(Self {
            directory,
            generation,
            manifest: (!manifest.is_empty()).then_some(manifest),
            backing,
        })
    }

    fn publish(&mut self, manifest: &[u8], backing: &HashMap<Vec<u8>, Vec<u8>>) -> io::Result<()> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("cabinet snapshot generation exhausted"))?;
        let bytes = encode_snapshot(generation, manifest, backing)?;
        let stem = format!("candidate-{generation:020}");
        let temporary = self.directory.join(format!("{stem}.tmp"));
        let committed = self.directory.join(format!("{stem}.bin"));

        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &committed)?;

        self.generation = generation;
        self.manifest = Some(manifest.to_vec());
        self.backing = backing.clone();
        Ok(())
    }
}

impl PartialPersistenceProvider for DirectoryProvider {
    fn load_manifest(&mut self) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.manifest.clone())
    }

    fn load_backing(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.backing.get(key).cloned())
    }

    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        backing_replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), RuntimeError> {
        let mut candidate = self.backing.clone();
        for (key, payload) in backing_replacements {
            candidate.insert(key.clone(), payload.clone());
        }
        self.publish(manifest, &candidate)
            .map_err(|error| RuntimeError::new(format!("cabinet persistence failed: {error}")))
    }
}

fn encode_snapshot(
    generation: u64,
    manifest: &[u8],
    backing: &HashMap<Vec<u8>, Vec<u8>>,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(SNAPSHOT_MAGIC);
    bytes.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&generation.to_le_bytes());
    push_bytes(&mut bytes, manifest)?;

    let count = u64::try_from(backing.len())
        .map_err(|_| io::Error::other("too many cabinet backing entries"))?;
    bytes.extend_from_slice(&count.to_le_bytes());
    let mut entries = backing.iter().collect::<Vec<_>>();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (key, payload) in entries {
        push_bytes(&mut bytes, key)?;
        push_bytes(&mut bytes, payload)?;
    }
    Ok(bytes)
}

fn decode_snapshot(bytes: &[u8]) -> io::Result<(u64, Vec<u8>, HashMap<Vec<u8>, Vec<u8>>)> {
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0u8; 8];
    cursor.read_exact(&mut magic)?;
    if &magic != SNAPSHOT_MAGIC {
        return Err(io::Error::other("invalid cabinet snapshot magic"));
    }
    let version = read_u32(&mut cursor)?;
    if version != SNAPSHOT_VERSION {
        return Err(io::Error::other(format!(
            "unsupported cabinet snapshot version {version}"
        )));
    }
    let generation = read_u64(&mut cursor)?;
    let manifest = read_bytes(&mut cursor)?;
    let count = read_u64(&mut cursor)?;
    let mut backing = HashMap::new();
    for _ in 0..count {
        let key = read_bytes(&mut cursor)?;
        let payload = read_bytes(&mut cursor)?;
        if backing.insert(key, payload).is_some() {
            return Err(io::Error::other("duplicate cabinet backing key"));
        }
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(io::Error::other("trailing cabinet snapshot bytes"));
    }
    Ok((generation, manifest, backing))
}

fn push_bytes(output: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    let len = u64::try_from(value.len())
        .map_err(|_| io::Error::other("cabinet snapshot field is too large"))?;
    output.extend_from_slice(&len.to_le_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_bytes(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let len = read_u64(reader)?;
    let len = usize::try_from(len).map_err(|_| io::Error::other("snapshot field is too large"))?;
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
