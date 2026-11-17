fn main() {
    let code = match hession_cli::dispatch_env() {
        hession_cli::Dispatch::Handled(code) => code,
        hession_cli::Dispatch::Gui => {
            eprintln!("This headless development binary has no GUI. Run the installed hession executable to open the desktop.");
            2
        }
        hession_cli::Dispatch::Index(_) => {
            eprintln!("Indexing is supplied by the installed hession application adapter. Use the installed application for native-store indexing.");
            2
        }
    };
    std::process::exit(code);
}
