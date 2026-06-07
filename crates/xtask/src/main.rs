use std::env;
use std::path::{Path, PathBuf};

mod fdr;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err("usage: cargo xtask generate-fdr-schema [--check]".to_owned());
    };
    let root = workspace_root()?;

    match command.as_str() {
        "generate-fdr-schema" => fdr::generate_schema(&root, args),
        _ => Err(format!("unknown xtask command {command:?}")),
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|err| format!("read cwd: {err}"))?;
    if cwd.join("Cargo.toml").exists() && cwd.join("crates").exists() {
        return Ok(cwd);
    }
    cwd.parent()
        .filter(|parent| parent.join("Cargo.toml").exists() && parent.join("crates").exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| "run xtask from the workspace root or an immediate child".to_owned())
}
