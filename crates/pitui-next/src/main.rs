use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("pitui-next: {error}");
            return ExitCode::FAILURE;
        }
    };
    let paths = pitui_next::repository_paths_from_args(&cwd, env::args_os().skip(1));
    match pitui_next::NextApp::open_from(&cwd, paths) {
        Ok(app) => match pitui_next::run_terminal(app) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pitui-next: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("pitui-next: {error}");
            ExitCode::FAILURE
        }
    }
}
