use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("pitui: {error}");
            return ExitCode::FAILURE;
        }
    };
    let paths = pitui::repository_paths_from_args(&cwd, env::args_os().skip(1));
    match pitui::App::open_from(&cwd, paths) {
        Ok(app) => match pitui::run_terminal(app) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pitui: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("pitui: {error}");
            ExitCode::FAILURE
        }
    }
}
