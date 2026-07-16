//! Pitui is a deliberately small Git browser built around a strict boundary:
//! terminal code emits actions, application code owns state, and only the Git
//! worker is allowed to invoke `git`.

pub mod app;
pub mod domain;
pub mod git;
pub mod tui;

use std::{
    collections::HashSet,
    env,
    error::Error,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Resolves positional repository arguments while preserving their order.
/// Relative paths are interpreted from the process working directory.
pub fn repository_paths_from_args(
    cwd: &Path,
    args: impl IntoIterator<Item = OsString>,
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut paths = args
        .into_iter()
        .filter(|argument| argument != "--")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        paths.push(cwd.to_path_buf());
    }
    paths
}

pub fn run_with_repository_paths(paths: Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    let bus = git::GitCommandBus::spawn();
    let app = app::App::new(bus, paths);
    tui::run(app)
}

/// Runs Pitui for every repository path passed on the command line, or the
/// current working directory when no path is supplied.
pub fn run() -> Result<(), Box<dyn Error>> {
    let cwd = env::current_dir()?;
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && matches!(args[0].to_str(), Some("-h" | "--help")) {
        println!(
            "Usage: pitui [REPOSITORY ...]\n\nDefaults to the current directory.\nBackend log: {}\nOverride the log path with PITUI_LOG.",
            git::default_backend_log_path().display()
        );
        return Ok(());
    }
    run_with_repository_paths(repository_paths_from_args(&cwd, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_deduplicates_and_defaults_repository_paths() {
        let cwd = Path::new("/work");
        assert_eq!(repository_paths_from_args(cwd, Vec::new()), vec![cwd]);
        assert_eq!(
            repository_paths_from_args(
                cwd,
                [
                    OsString::from("one"),
                    OsString::from("/two"),
                    OsString::from("one")
                ]
            ),
            vec![PathBuf::from("/work/one"), PathBuf::from("/two")]
        );
    }
}
