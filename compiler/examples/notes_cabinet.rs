use std::{
    collections::HashMap,
    error::Error,
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
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
const BROWSER_ADDR: &str = "127.0.0.1:7878";
const MAX_HTTP_REQUEST_BYTES: usize = 1024 * 1024;

type CabinetRuntime = PartialPersistentRuntime<DirectoryProvider>;
type SharedCabinetRuntime = Arc<Mutex<CabinetRuntime>>;
type AppResult<T> = Result<T, Box<dyn Error>>;

fn assert_send<T: Send>() {}

// Host boundary: this example may retain terminal input/status state, but it must not
// retain modeled identities, structural membership, application selection, parentage,
// trash state, restore semantics, or any other authoritative Cabinet application fact.
fn main() -> AppResult<()> {
    assert_send::<CabinetRuntime>();

    let checked = checked_source()?;
    let world_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".elanu-cabinet"));

    let provider = DirectoryProvider::open(world_dir.clone())?;
    let runtime = Arc::new(Mutex::new(PartialPersistentRuntime::open(
        checked.clone(),
        provider,
    )?));

    with_runtime(&runtime, |runtime| -> AppResult<()> {
        if !bool_value(runtime, "initialized")? {
            runtime.run_action("initialize")?;
        }

        materialize_visible(runtime)?;
        Ok(())
    })?;

    start_browser_bridge(Arc::clone(&runtime))?;

    println!("Browser view: http://{BROWSER_ADDR}");

    let mut status = format!("Notes Cabinet opened. Browser view: http://{BROWSER_ADDR}");

    loop {
        with_runtime(&runtime, |runtime| render(runtime, &status))?;

        // Do not hold the runtime lock while waiting for terminal input.
        let command = prompt("cabinet")?;

        status = match command.trim() {
            "q" | "quit" => break,
            "h" | "home" => with_runtime(&runtime, |runtime| run_visible(runtime, "selectHome")),
            "ot" | "open-trash" => {
                with_runtime(&runtime, |runtime| run_visible(runtime, "openTrash"))
            }
            "f" | "folder" => select_index_shared(&runtime, "selectFolder", "child folder index"),
            "n" | "note" => select_index_shared(&runtime, "selectNote", "note index"),
            "nf" | "new-folder" => create_folder_shared(&runtime),
            "nn" | "new-note" => create_note_shared(&runtime),
            "rf" | "rename-folder" => rename_folder_shared(&runtime),
            "e" | "edit" => edit_note_shared(&runtime),
            "t" | "trash" => with_runtime(&runtime, |runtime| run(runtime, "trashSelectedNote")),
            "r" | "restore" => with_runtime(&runtime, |runtime| {
                run_visible(runtime, "restoreSelectedNote")
            }),
            "d" | "delete" => with_runtime(&runtime, |runtime| {
                run(runtime, "permanentlyDeleteSelectedNote")
            }),
            "x" | "fail" => demonstrate_rollback_shared(&runtime),
            "restart" => restart_shared(&runtime, &checked, &world_dir),
            "?" | "help" => String::from("Commands are shown below."),
            "" => String::new(),
            other => format!("Unknown command: {other}"),
        };
    }

    println!("Cabinet closed. Durable world remains in the selected world directory.");
    Ok(())
}

fn with_runtime<T>(
    shared: &SharedCabinetRuntime,
    operation: impl FnOnce(&mut CabinetRuntime) -> T,
) -> T {
    let mut runtime = shared
        .lock()
        .expect("Cabinet runtime mutex should not be poisoned");
    operation(&mut runtime)
}

fn start_browser_bridge(shared: SharedCabinetRuntime) -> io::Result<()> {
    let listener = TcpListener::bind(BROWSER_ADDR)?;

    thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    if let Err(error) = handle_browser_request(&shared, stream) {
                        eprintln!("Cabinet browser request failed: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("Cabinet browser connection failed: {error}");
                }
            }
        }
    });

    Ok(())
}

fn handle_browser_request(shared: &SharedCabinetRuntime, mut stream: TcpStream) -> io::Result<()> {
    let request = read_http_request(&mut stream)?;

    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();

    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => {
            let page = with_runtime(shared, browser_page);

            match page {
                Ok(body) => write_http_response(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    &body,
                    &[],
                ),
                Err(error) => write_http_response(
                    &mut stream,
                    "500 Internal Server Error",
                    "text/plain; charset=utf-8",
                    &format!("Cabinet observation failed: {error}"),
                    &[],
                ),
            }
        }

        ("POST", "/action") => {
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or_default();

            match invoke_browser_action(shared, body) {
                Ok(()) => write_http_response(
                    &mut stream,
                    "303 See Other",
                    "text/plain; charset=utf-8",
                    "",
                    &[("Location", "/")],
                ),
                Err(error) => write_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "text/plain; charset=utf-8",
                    &error,
                    &[],
                ),
            }
        }

        ("GET", "/favicon.ico") => {
            write_http_response(&mut stream, "204 No Content", "text/plain", "", &[])
        }

        ("GET", _) => write_http_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "Not found.",
            &[],
        ),

        _ => write_http_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            "Unsupported request.",
            &[],
        ),
    }
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut expected_total = None;

    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        bytes.extend_from_slice(&buffer[..read]);

        if bytes.len() > MAX_HTTP_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP request exceeds Cabinet bridge limit",
            ));
        }

        if expected_total.is_none() {
            if let Some(header_end) = find_header_end(&bytes) {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = parse_content_length(&headers)?;
                let total = header_end
                    .checked_add(4)
                    .and_then(|value| value.checked_add(content_length))
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "HTTP request size overflow")
                    })?;

                if total > MAX_HTTP_REQUEST_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "HTTP request exceeds Cabinet bridge limit",
                    ));
                }

                expected_total = Some(total);
            }
        }

        if let Some(total) = expected_total {
            if bytes.len() >= total {
                bytes.truncate(total);
                break;
            }
        }
    }

    let header_end = find_header_end(&bytes).ok_or_else(|| {
        io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete HTTP request headers")
    })?;
    let total = expected_total.unwrap_or(header_end + 4);
    if bytes.len() < total {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete HTTP request body",
        ));
    }

    String::from_utf8(bytes)
        .map_err(|error| io::Error::other(format!("invalid HTTP request: {error}")))
}

fn parse_content_length(headers: &str) -> io::Result<usize> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };

        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length header")
            });
        }
    }

    Ok(0)
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn invoke_browser_action(shared: &SharedCabinetRuntime, body: &str) -> Result<(), String> {
    let fields = parse_form_urlencoded(body)?;
    let action = fields
        .get("action")
        .ok_or_else(|| String::from("missing action"))?
        .as_str();

    match action {
        "selectFolder" | "selectNote" => {
            let argument = fields
                .get("argument")
                .ok_or_else(|| String::from("missing argument"))?
                .parse::<i64>()
                .map_err(|_| String::from("invalid integer argument"))?;

            with_runtime(shared, |runtime| {
                runtime
                    .run_action_with_values(action, &[Value::Int(argument)])
                    .map_err(|error| error.to_string())?;

                materialize_visible(runtime).map_err(|error| error.to_string())
            })
        }
        "selectHome" | "openTrash" => with_runtime(shared, |runtime| {
            runtime.run_action(action).map_err(|error| error.to_string())?;
            materialize_visible(runtime).map_err(|error| error.to_string())
        }),
        "editSelectedNote" => {
            let title = fields
                .get("title")
                .ok_or_else(|| String::from("missing title"))?
                .clone();
            let body = fields
                .get("body")
                .ok_or_else(|| String::from("missing body"))?
                .clone();

            with_runtime(shared, |runtime| {
                runtime
                    .run_action_with_values(
                        "editSelectedNote",
                        &[Value::String(title), Value::String(body)],
                    )
                    .map_err(|error| error.to_string())?;

                materialize_visible(runtime).map_err(|error| error.to_string())
            })
        }
        _ => Err(format!("unsupported action '{action}'")),
    }
}

fn parse_form_urlencoded(body: &str) -> Result<HashMap<String, String>, String> {
    let mut fields = HashMap::new();

    if body.is_empty() {
        return Ok(fields);
    }

    for field in body.split('&') {
        let (name, value) = field
            .split_once('=')
            .ok_or_else(|| String::from("malformed form field"))?;
        let name = decode_form_component(name)?;
        let value = decode_form_component(value)?;

        if fields.insert(name.clone(), value).is_some() {
            return Err(format!("duplicate form field '{name}'"));
        }
    }

    Ok(fields)
}

fn decode_form_component(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(String::from("incomplete percent escape in form data"));
                }

                let high = hex_digit(bytes[index + 1])
                    .ok_or_else(|| String::from("invalid percent escape in form data"))?;
                let low = hex_digit(bytes[index + 2])
                    .ok_or_else(|| String::from("invalid percent escape in form data"))?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).map_err(|_| String::from("form data is not valid UTF-8"))
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> io::Result<()> {
    let body = body.as_bytes();

    write!(
        stream,
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n",
        body.len()
    )?;

    for (name, value) in extra_headers {
        write!(stream, "{name}: {value}\r\n")?;
    }

    write!(stream, "\r\n")?;
    stream.write_all(body)?;
    stream.flush()
}

fn browser_page(runtime: &mut CabinetRuntime) -> AppResult<String> {
    let folder = string_value(runtime, "selectedFolderName")?;
    let note_present = bool_value(runtime, "selectedNotePresent")?;

    let note_title = if note_present {
        string_value(runtime, "selectedNoteTitle")?
    } else {
        String::new()
    };

    let note_body = if note_present {
        string_value(runtime, "selectedNoteBody")?
    } else {
        String::new()
    };

    let in_trash = if note_present {
        bool_value(runtime, "selectedNoteInTrash")?
    } else {
        false
    };

    let folders = designation_member_strings(runtime, "selectedFolder", "folders", "name")?;
    let notes = designation_member_strings(runtime, "selectedFolder", "notes", "title")?;

    let mut contents = String::new();

    if folders.is_empty() && notes.is_empty() {
        contents.push_str("<li class=\"empty\">&lt;empty&gt;</li>");
    } else {
        for (index, name) in folders.iter().enumerate() {
            contents.push_str(&format!(
                r#"
        <li>
            <form class="row-action" method="post" action="/action">
                <input type="hidden" name="action" value="selectFolder">
                <input type="hidden" name="argument" value="{index}">
                <button type="submit">
                    <span class="index">f{index}</span> 📁 {name}/
                </button>
            </form>
        </li>
        "#,
                name = escape_html(name),
            ));
        }

        for (index, title) in notes.iter().enumerate() {
            contents.push_str(&format!(
                r#"
        <li>
            <form class="row-action" method="post" action="/action">
                <input type="hidden" name="action" value="selectNote">
                <input type="hidden" name="argument" value="{index}">
                <button type="submit">
                    <span class="index">n{index}</span> 📝 {title}
                </button>
            </form>
        </li>
        "#,
                title = escape_html(title),
            ));
        }
    }

    let selected_note = if note_present {
        let trash_badge = if in_trash {
            "<span class=\"badge\">TRASHED</span>"
        } else {
            ""
        };

        format!(
            r#"
            <section>
                <h2>Selected note {trash_badge}</h2>
                <form class="edit-note" method="post" action="/action">
                    <input type="hidden" name="action" value="editSelectedNote">
                    <label>
                        Title
                        <input type="text" name="title" value="{title}">
                    </label>
                    <label>
                        Body
                        <textarea name="body" rows="10">{body}</textarea>
                    </label>
                    <button type="submit">Save note</button>
                </form>
            </section>
            "#,
            title = escape_html(&note_title),
            body = escape_html(&note_body),
        )
    } else {
        String::from(
            r#"
            <section>
                <h2>Selected note</h2>
                <p class="empty">&lt;none selected&gt;</p>
            </section>
            "#,
        )
    };

    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Elanu Notes Cabinet</title>
    <style>
        :root {{
            font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
            color-scheme: light dark;
        }}

        body {{
            max-width: 900px;
            margin: 3rem auto;
            padding: 0 1.5rem;
            line-height: 1.5;
        }}

        header {{
            display: flex;
            align-items: baseline;
            justify-content: space-between;
            gap: 1rem;
            border-bottom: 1px solid;
            padding-bottom: 1rem;
            margin-bottom: 2rem;
        }}

        h1, h2, h3 {{
            margin-top: 0;
        }}

        section {{
            margin: 2rem 0;
        }}

        ul {{
            list-style: none;
            padding: 0;
        }}

        li {{
            padding: 0.35rem 0;
        }}

        form {{
            margin: 0;
        }}

        .index {{
            display: inline-block;
            min-width: 2.5rem;
            opacity: 0.65;
            font-family: monospace;
        }}

        .badge {{
            font-size: 0.7rem;
            border: 1px solid;
            border-radius: 0.3rem;
            padding: 0.15rem 0.35rem;
            margin-left: 0.5rem;
        }}

        .empty {{
            opacity: 0.65;
        }}

        .places {{
            display: flex;
            gap: 1rem;
            flex-wrap: wrap;
        }}

        .row-action button,
        .place-action button {{
            font: inherit;
            color: inherit;
            background: none;
            border: 0;
            padding: 0.35rem 0;
            cursor: pointer;
            text-align: left;
        }}

        .row-action button:hover,
        .place-action button:hover {{
            text-decoration: underline;
        }}

        .edit-note {{
            display: grid;
            gap: 1rem;
        }}

        .edit-note label {{
            display: grid;
            gap: 0.35rem;
            font-weight: 600;
        }}

        .edit-note input[type="text"],
        .edit-note textarea {{
            width: 100%;
            box-sizing: border-box;
            font: inherit;
            color: inherit;
            background: transparent;
            border: 1px solid;
            border-radius: 0.4rem;
            padding: 0.65rem;
        }}

        .edit-note textarea {{
            resize: vertical;
        }}

        .edit-note button {{
            width: fit-content;
            font: inherit;
            color: inherit;
            background: transparent;
            border: 1px solid;
            border-radius: 0.4rem;
            padding: 0.5rem 0.8rem;
            cursor: pointer;
        }}

        footer {{
            border-top: 1px solid;
            margin-top: 3rem;
            padding-top: 1rem;
            opacity: 0.65;
            font-size: 0.9rem;
        }}

        a {{
            white-space: nowrap;
        }}
    </style>
</head>
<body>
    <header>
        <div>
            <h1>Elanu Notes Cabinet</h1>
            <div>Interactive browser surface</div>
        </div>
        <a href="/">Refresh</a>
    </header>

    <main>
        <section>
            <h2>Places</h2>
            <div class="places">
                <form class="place-action" method="post" action="/action">
                    <input type="hidden" name="action" value="selectHome">
                    <button type="submit">Home / Notes</button>
                </form>
                <form class="place-action" method="post" action="/action">
                    <input type="hidden" name="action" value="openTrash">
                    <button type="submit">Trash</button>
                </form>
            </div>
        </section>

        <section>
            <h2>Selected folder</h2>
            <h3>{folder}</h3>

            <ul>
                {contents}
            </ul>
        </section>

        {selected_note}
    </main>

    <footer>
        One native Elanu runtime. This browser and the terminal surface observe
        and mutate the same committed Cabinet state.
    </footer>
</body>
</html>
"#,
        folder = escape_html(&folder),
    ))
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }

    escaped
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

fn select_index_shared(shared: &SharedCabinetRuntime, action: &str, label: &str) -> String {
    let value = match prompt(label) {
        Ok(value) => value,
        Err(error) => return format!("Input failed: {error}"),
    };

    let index = match value.parse::<i64>() {
        Ok(index) => index,
        Err(_) => return format!("'{value}' is not an integer index."),
    };

    with_runtime(shared, |runtime| {
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
    })
}

fn create_folder_shared(shared: &SharedCabinetRuntime) -> String {
    let name = match prompt("folder name") {
        Ok(name) => name,
        Err(error) => return format!("Input failed: {error}"),
    };

    with_runtime(shared, |runtime| {
        match runtime.run_action_with_values("createFolder", &[Value::String(name)]) {
            Ok(()) => String::from("Created and selected folder."),
            Err(error) => format!("createFolder failed: {error}"),
        }
    })
}

fn create_note_shared(shared: &SharedCabinetRuntime) -> String {
    let title = match prompt("title") {
        Ok(title) => title,
        Err(error) => return format!("Input failed: {error}"),
    };

    let body = match prompt("body") {
        Ok(body) => body,
        Err(error) => return format!("Input failed: {error}"),
    };

    with_runtime(shared, |runtime| {
        match runtime
            .run_action_with_values("createNote", &[Value::String(title), Value::String(body)])
        {
            Ok(()) => String::from("Created and selected note."),
            Err(error) => format!("createNote failed: {error}"),
        }
    })
}

fn rename_folder_shared(shared: &SharedCabinetRuntime) -> String {
    let name = match prompt("new folder name") {
        Ok(name) => name,
        Err(error) => return format!("Input failed: {error}"),
    };

    with_runtime(shared, |runtime| {
        match runtime.run_action_with_values("renameSelectedFolder", &[Value::String(name)]) {
            Ok(()) => String::from("Renamed selected folder."),
            Err(error) => format!("renameSelectedFolder failed: {error}"),
        }
    })
}

fn edit_note_shared(shared: &SharedCabinetRuntime) -> String {
    let title = match prompt("new title") {
        Ok(title) => title,
        Err(error) => return format!("Input failed: {error}"),
    };

    let body = match prompt("new body") {
        Ok(body) => body,
        Err(error) => return format!("Input failed: {error}"),
    };

    with_runtime(shared, |runtime| {
        match runtime.run_action_with_values(
            "editSelectedNote",
            &[Value::String(title), Value::String(body)],
        ) {
            Ok(()) => String::from("Edited selected note."),
            Err(error) => format!("editSelectedNote failed: {error}"),
        }
    })
}

fn demonstrate_rollback_shared(shared: &SharedCabinetRuntime) -> String {
    let before = match with_runtime(shared, |runtime| string_value(runtime, "selectedNoteTitle")) {
        Ok(value) => value,
        Err(error) => return format!("Could not read prior title: {error}"),
    };

    let attempted = match prompt("title to roll back") {
        Ok(value) => value,
        Err(error) => return format!("Input failed: {error}"),
    };

    with_runtime(shared, |runtime| {
        match runtime.run_action_with_values("failEditSelectedNote", &[Value::String(attempted)]) {
            Ok(()) => String::from("Unexpectedly committed failing edit."),
            Err(error) => match string_value(runtime, "selectedNoteTitle") {
                Ok(after) if after == before => {
                    format!("Action failed as requested ({error}); title remained '{after}'.")
                }
                Ok(after) => format!("Rollback proof failed: title changed to '{after}'."),
                Err(read_error) => {
                    format!("Action failed ({error}); reread failed: {read_error}")
                }
            },
        }
    })
}

fn restart_shared(
    shared: &SharedCabinetRuntime,
    checked: &CheckedSource,
    world_dir: &PathBuf,
) -> String {
    let mut current = shared
        .lock()
        .expect("Cabinet runtime mutex should not be poisoned");

    let provider = match DirectoryProvider::open(world_dir.clone()) {
        Ok(provider) => provider,
        Err(error) => return format!("Restart failed: {error}"),
    };

    let mut restarted = match PartialPersistentRuntime::open(checked.clone(), provider) {
        Ok(runtime) => runtime,
        Err(error) => return format!("Restart failed: {error}"),
    };

    if let Err(error) = materialize_visible(&mut restarted) {
        return format!("Restart failed: {error}");
    }

    *current = restarted;
    String::from("Reopened the same persisted Elanu world.")
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
