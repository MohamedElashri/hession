//! Headless command dispatch. The app consumes Index requests before any webview.
mod args;
mod cli;
mod error;
pub mod mcp;
mod transport;
use clap::error::ErrorKind;
pub use error::CliError;
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexRequest {
    pub db_path: PathBuf,
    pub watch: bool,
    pub agent: Option<String>,
    pub json: bool,
}
#[derive(Debug)]
pub enum Dispatch {
    Gui,
    Index(IndexRequest),
    Handled(i32),
}

pub fn dispatch_env() -> Dispatch {
    dispatch(std::env::args_os())
}
/// No arguments returns Gui without constructing a runtime, opening a DB or
/// initializing an app. All explicit commands are handled headlessly.
pub fn dispatch(arguments: impl IntoIterator<Item = OsString>) -> Dispatch {
    let arguments = arguments.into_iter().take(2049).collect::<Vec<_>>();
    if arguments.len() <= 1 {
        return Dispatch::Gui;
    }
    let json_requested = arguments.iter().any(|value| value == "--json");
    if arguments.len() > 2048
        || arguments
            .iter()
            .map(|s| s.as_encoded_bytes().len())
            .sum::<usize>()
            > 64 * 1024
    {
        let error = CliError::new(2, "usage", "Command line exceeds its resource limit");
        if json_requested {
            eprintln!("{}", serde_json::to_string(&error).unwrap());
        } else {
            eprintln!("{error}");
        }
        return Dispatch::Handled(2);
    }
    let parsed = match args::command().try_get_matches_from(arguments) {
        Ok(v) => v,
        Err(e) => {
            let code = if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                0
            } else {
                2
            };
            if json_requested && code != 0 {
                eprintln!(
                    "{}",
                    serde_json::to_string(&CliError::new(code, "usage", e.to_string())).unwrap()
                );
            } else {
                let _ = e.print();
            }
            return Dispatch::Handled(code);
        }
    };
    let json = parsed.get_flag("json");
    let db_path = parsed
        .get_one::<PathBuf>("database")
        .cloned()
        .unwrap_or_else(hession_core::db::db_path);
    if let Some(("index", index)) = parsed.subcommand() {
        return Dispatch::Index(IndexRequest {
            db_path,
            watch: index.get_flag("watch"),
            agent: index.get_one::<String>("agent").cloned(),
            json,
        });
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Could not start headless runtime: {e}");
            return Dispatch::Handled(3);
        }
    };
    let result = runtime.block_on(cli::execute(&parsed, db_path, json));
    // macOS stdio may have an uncancellable blocking stdin read. The app exits
    // after Handled; shutdown must never hold the process indefinitely.
    runtime.shutdown_timeout(std::time::Duration::from_millis(250));
    match result {
        Ok(code) => Dispatch::Handled(code),
        Err(error) => Dispatch::Handled(report_error(&error, json)),
    }
}

/// Await SIGINT/SIGTERM without constructing a webview. The app index adapter
/// can await this on its existing asynchronous runtime and then stop its worker.
pub async fn wait_for_shutdown() -> i32 {
    cli::shutdown_signal().await
}

/// Write one bounded success value. JSON mode is one line, suitable for watch
/// JSONL; human mode pretty-prints while escaping terminal control characters.
pub fn write_output(value: &serde_json::Value, json: bool) -> Result<(), CliError> {
    cli::output(value, json)
}
/// Await one bounded stdout worker without blocking the asynchronous signal
/// driver. The caller may select this future against shutdown, then exit after
/// stopping its catalog worker. A cancelled OS write owns the sole output permit
/// until it actually finishes; subsequent calls fail busy instead of queuing.
pub async fn write_output_async(value: serde_json::Value, json: bool) -> Result<(), CliError> {
    static OUTPUT_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let permit = OUTPUT_WORKERS
        .try_acquire()
        .map_err(|_| CliError::new(6, "output_busy", "A previous stdout write is still blocked"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        write_output(&value, json)
    })
    .await
    .map_err(|_| CliError::new(3, "output", "Output worker failed"))?
}
/// Emit a stable error on stderr and return its intended process exit code.
/// No success/error helper writes diagnostics to stdout.
pub fn report_error(error: &CliError, json: bool) -> i32 {
    if json {
        eprintln!(
            "{}",
            serde_json::to_string(error).unwrap_or_else(|_| {
                "{\"code\":3,\"kind\":\"error\",\"message\":\"Error serialization failed\"}".into()
            })
        );
    } else {
        eprintln!("{}", cli::terminal_safe(&error.to_string()));
    }
    error.code
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(args: &[&str]) -> Dispatch {
        dispatch(args.iter().map(OsString::from))
    }
    #[test]
    fn no_arguments_choose_gui_without_opening_a_library() {
        assert!(matches!(run(&["hession"]), Dispatch::Gui));
        assert!(matches!(dispatch(Vec::<OsString>::new()), Dispatch::Gui));
    }
    #[test]
    fn index_actions_are_parsed_without_opening_a_database() {
        let parsed = run(&[
            "hession",
            "--database",
            "/does-not-exist/index.sqlite",
            "index",
            "--watch",
            "--agent",
            "codex",
            "--json",
        ]);
        assert!(
            matches!(parsed,Dispatch::Index(IndexRequest{db_path,watch:true,agent:Some(agent),json:true}) if db_path==std::path::Path::new("/does-not-exist/index.sqlite")&&agent=="codex")
        );
        assert!(matches!(
            run(&["hession", "index", "--once"]),
            Dispatch::Index(IndexRequest { watch: false, .. })
        ));
    }
    #[test]
    fn index_requires_exactly_one_explicit_mode() {
        for args in [
            vec!["hession", "index"],
            vec!["hession", "index", "--once", "--watch"],
            vec!["hession", "index", "--once", "--agent", "unknown"],
        ] {
            assert!(matches!(run(&args), Dispatch::Handled(2)));
        }
    }
}
