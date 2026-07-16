fn main() {
    if let Err(error) = pitui::run() {
        eprintln!("pitui: {error}");
        std::process::exit(1);
    }
}
