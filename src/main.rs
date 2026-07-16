fn main() {
    if let Err(error) = pitui::run() {
        eprintln!("pitui: {error}");
        std::process::exit(
            if error.downcast_ref::<pitui::config::ConfigError>().is_some() {
                2
            } else {
                1
            },
        );
    }
}
