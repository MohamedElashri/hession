fn main() {
    match hession_cli::dispatch_env() {
        hession_cli::Dispatch::Gui => app_lib::run(),
        hession_cli::Dispatch::Handled(code) => std::process::exit(code),
        hession_cli::Dispatch::Index(request) => {
            std::process::exit(app_lib::headless_index::run(request))
        }
    }
}
