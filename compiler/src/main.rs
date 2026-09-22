use std::{env, fs, process};

use elanu_compiler::{
    check_source, check_source_with_runtime_models, parse_source, runtime::Runtime,
};

fn main() {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| usage_and_exit());
    let path = args.next().unwrap_or_else(|| usage_and_exit());
    let action = args.next();

    if args.next().is_some() {
        usage_and_exit();
    }

    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("elanu: could not read '{path}': {error}");
            process::exit(1);
        }
    };

    match command.as_str() {
        "parse" => {
            if action.is_some() {
                usage_and_exit();
            }
            match parse_source(&source) {
                Ok(program) => print!("{program}"),
                Err(errors) => report_errors(errors),
            }
        }
        "check" => {
            if action.is_some() {
                usage_and_exit();
            }
            match check_source(&source) {
                Ok(_) => println!("OK"),
                Err(errors) => report_errors(errors),
            }
        }
        "run" => match check_source_with_runtime_models(&source) {
            Ok(program) => {
                let mut runtime = match Runtime::from_checked_source(&program) {
                    Ok(runtime) => runtime,
                    Err(error) => report_runtime_error(error),
                };

                if let Some(action) = action {
                    if let Err(error) = runtime.run_action(&action) {
                        report_runtime_error(error);
                    }
                }

                match runtime.snapshot() {
                    Ok(entries) => {
                        for entry in entries {
                            println!("{} {} = {}", entry.kind, entry.name, entry.value);
                        }
                    }
                    Err(error) => report_runtime_error(error),
                }
            }
            Err(errors) => report_errors(errors),
        },
        _ => usage_and_exit(),
    }
}

fn report_errors(errors: Vec<elanu_compiler::diagnostic::Diagnostic>) -> ! {
    for error in errors {
        eprintln!("{error}");
    }
    process::exit(1)
}

fn report_runtime_error(error: elanu_compiler::runtime::RuntimeError) -> ! {
    eprintln!("elanu runtime: {error}");
    process::exit(1)
}

fn usage_and_exit() -> ! {
    eprintln!("usage: elanu <parse|check> <file.elnu>");
    eprintln!("       elanu run <file.elnu> [action]");
    process::exit(2);
}
