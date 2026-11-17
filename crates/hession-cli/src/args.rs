use clap::{Arg, ArgAction, ArgGroup, Command};
fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::SetTrue)
}
fn project() -> Arg {
    Arg::new("project")
        .long("project")
        .help("Exact indexed project path or workspace:ID")
        .num_args(1)
}
fn paging(command: Command) -> Command {
    command
        .arg(
            Arg::new("limit")
                .long("limit")
                .default_value("20")
                .value_parser(clap::value_parser!(u32).range(1..=100)),
        )
        .arg(
            Arg::new("cursor")
                .long("cursor")
                .help("Opaque cursor from the preceding page"),
        )
}
fn id() -> Arg {
    Arg::new("id")
        .required(true)
        .value_parser(clap::value_parser!(i64).range(1..))
}
fn path() -> Arg {
    Arg::new("path").required(true)
}
pub fn command() -> Command {
    Command::new("hession").version(env!("CARGO_PKG_VERSION")).about("Local coding-agent history; no arguments opens the desktop")
    .arg(Arg::new("database").long("database").visible_alias("db").global(true).value_parser(clap::value_parser!(std::path::PathBuf)).help("Existing library database (default: HESSION_DATA_DIR/sessions.sqlite)"))
    .arg(flag("json","Machine-readable JSON output; errors go to stderr").global(true))
    .subcommand(Command::new("status").about("Read library revision, counts and delta availability"))
    .subcommand(Command::new("index").about("Explicit local indexing through the desktop application's importer")
        .arg(flag("once","Index changed local sources once"))
        .arg(flag("watch","Observe local sources in the foreground until SIGINT/SIGTERM"))
        .group(ArgGroup::new("mode").args(["once","watch"]).required(true).multiple(false))
        .arg(Arg::new("agent").long("agent").value_parser(["claude","codex","opencode","cline","pi","commandcode","muse","hermes","antigravity"])))
    .subcommand(Command::new("sessions").subcommand_required(true).arg_required_else_help(true)
        .subcommand(paging(Command::new("list").about("List recent sessions").arg(project()).arg(Arg::new("agent").long("agent"))))
        .subcommand(paging(Command::new("show").about("Show safe user/assistant conversation text").arg(id()).arg(project())
            .arg(Arg::new("format").long("format").default_value("text").value_parser(["text","markdown","json"]))
            .arg(flag("metadata-only","Do not fetch conversation text"))))
        .subcommand(Command::new("follow").about("Follow durable recorded changes; does not start an agent").arg(id()).arg(flag("jsonl","Emit bounded newline-delimited delta envelopes"))
            .arg(Arg::new("limit").long("limit").default_value("20").value_parser(clap::value_parser!(u32).range(1..=100)))))
    .subcommand(paging(Command::new("search").about("Structured developer search (AND, OR, phrases, exclusions, fields)")
        .arg(Arg::new("query").required(true).allow_hyphen_values(true)).arg(project())
        .arg(flag("metadata-only","Search metadata only, excluding transcript text"))))
    .subcommand(Command::new("projects").subcommand_required(true).arg_required_else_help(true).subcommand(paging(Command::new("list").about("List indexed project identities"))))
    .subcommand(Command::new("project").subcommand_required(true).arg_required_else_help(true).subcommand(Command::new("show").about("Show an exact indexed project").arg(Arg::new("project").required(true))))
    .subcommand(Command::new("files").subcommand_required(true).arg_required_else_help(true)
        .subcommand(paging(Command::new("history").about("Read recorded file evidence; never opens a file").arg(path()).arg(project().required(true))))
        .subcommand(paging(Command::new("list").about("List reported file paths").arg(project().required(true)).arg(Arg::new("filter").long("filter")))))
    .subcommand(Command::new("export").subcommand_required(true).arg_required_else_help(true)
        .subcommand(Command::new("project").about("Write a new portable project ZIP using safe defaults")
            .arg(Arg::new("project").required(true)).arg(Arg::new("format").long("format").default_value("zip").value_parser(["zip"]))
            .arg(Arg::new("output").long("output").required(true).value_parser(clap::value_parser!(std::path::PathBuf)))
            .arg(Arg::new("query").long("query").default_value("")).arg(flag("preview","Print the bounded preview without writing an archive"))
            .arg(flag("include-statistics","Include aggregate statistics")).arg(flag("include-git-metadata","Include recorded Git metadata"))
            .arg(flag("include-tool-bodies","Explicitly include recorded tool inputs and outputs"))
            .arg(flag("include-absolute-paths","Explicitly include machine paths"))
            .arg(flag("no-redact","Explicitly disable pattern redaction"))))
    .subcommand(Command::new("mcp").subcommand_required(true).arg_required_else_help(true)
        .subcommand(Command::new("serve").about("Serve seven scoped read-only MCP tools over stdio only")
            .arg(project().required(true).action(ArgAction::Append))
            .arg(flag("expose-transcripts","Owner permission to expose only safe user/assistant text; no tool bodies or reasoning"))))
}
