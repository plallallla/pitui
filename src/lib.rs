//! Pitui is a deliberately small Git browser built around a strict boundary:
//! terminal code emits actions, application code owns state, and only the Git
//! worker is allowed to invoke `git`.

pub mod app;
pub mod config;
pub mod domain;
pub mod git;
pub mod tui;

use std::{
    collections::HashSet,
    env,
    error::Error,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
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
    let config = Arc::new(config::ResolvedConfig::default());
    let bus = git::GitCommandBus::spawn_with_logging_config(&config.logging)?;
    let app = app::App::new_with_config(bus, paths, config);
    tui::run(app)
}

/// Runs Pitui for every repository path passed on the command line, or the
/// current working directory when no path is supplied.
pub fn run() -> Result<(), Box<dyn Error>> {
    let cwd = env::current_dir()?;
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let (repository_args, cli) = config::repository_args_without_config_flags(args)?;
    if cli.help {
        println!(
            "Usage: pitui [OPTIONS] [REPOSITORY ...]\n\n\
             Defaults to the current directory.\n\n\
             Options:\n  \
               --config <PATH>          Use an explicit global config\n  \
               --no-config              Ignore the global config file\n  \
               --check-config           Validate config without starting the TUI\n  \
               --print-config-path      Print the selected config path\n  \
               --print-effective-config Print merged, normalized configuration\n  \
               -h, --help               Show this help\n\n\
             Default config: {}\n\
             Default backend log: {}\n\
             Environment: PITUI_CONFIG, PITUI_LOG",
            config::default_config_path().display(),
            config::default_backend_log_path().display()
        );
        return Ok(());
    }

    let mut load_options = config::ConfigLoadOptions::from_environment();
    if cli.path.is_some() {
        load_options.path = cli.path.clone();
    }
    load_options.no_config = cli.no_config;
    if cli.print_path {
        let path = config::selected_config_path(&load_options)?.map_or_else(
            || String::from("<disabled>"),
            |path| path.display().to_string(),
        );
        println!("{path}");
        if !cli.check && !cli.print_effective {
            return Ok(());
        }
    }

    let resolved = Arc::new(config::load(&load_options)?);
    if cli.check {
        println!(
            "configuration valid: {}",
            resolved.source_path.as_ref().map_or_else(
                || "built-in defaults".into(),
                |path| path.display().to_string()
            )
        );
    }
    if cli.print_effective {
        print!("{}", resolved.effective_toml());
    }
    if cli.check || cli.print_effective {
        return Ok(());
    }

    let paths = repository_paths_from_args(&cwd, repository_args);
    let bus = git::GitCommandBus::spawn_with_logging_config(&resolved.logging)?;
    let app = app::App::new_with_config(bus, paths, resolved);
    tui::run(app)
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
