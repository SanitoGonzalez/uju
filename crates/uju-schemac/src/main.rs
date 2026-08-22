use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use clap::builder::PossibleValuesParser;
use uju_schema::backend::{self, BACKENDS, GeneratedFile};
use uju_schema::compile;

/// The extension directories are searched for.
const EXTENSION: &str = "uju";

#[derive(Parser, Debug)]
#[command(name = "ujuc", version, about = "Compile uju schemas")]
struct Args {
    /// Target to generate; `ir` emits the compiled IR as JSON
    #[arg(
        value_name = "BACKEND",
        value_parser = PossibleValuesParser::new(BACKENDS.iter().copied()),
    )]
    backend: String,

    /// Directory to write generated files into
    #[arg(short = 'o', long = "out", value_name = "DIR", default_value = ".")]
    out: PathBuf,

    /// Compile and report what would be written, without writing it
    #[arg(long)]
    dry_run: bool,

    /// Schema files, or directories searched recursively for `*.uju`
    #[arg(value_name = "PATH", required = true)]
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ujuc: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<()> {
    let name = args.backend.as_str();
    let generator = backend::backend(name)
        .with_context(|| format!("no backend named `{name}`; known: {}", BACKENDS.join(", ")))?;

    let inputs = collect(&args.paths)?;
    if inputs.is_empty() {
        bail!("no `*.{EXTENSION}` files found");
    }
    let sources = inputs
        .iter()
        .map(|path| {
            fs::read_to_string(path).with_context(|| format!("reading `{}`", path.display()))
        })
        .collect::<Result<Vec<String>>>()?;

    let borrowed: Vec<&str> = sources.iter().map(String::as_str).collect();
    let schema = match compile(&borrowed) {
        Ok(schema) => schema,
        Err(mut diagnostics) => {
            diagnostics.sort_by_key(|diagnostic| (diagnostic.source.0, diagnostic.span.start));
            for diagnostic in &diagnostics {
                let index = diagnostic.source.0;
                let name = inputs[index].display().to_string();
                eprintln!("{}", diagnostic.render(&name, &sources[index]));
            }
            bail!("{} error(s); nothing was generated", diagnostics.len());
        }
    };

    let files = generator
        .generate(&schema)
        .with_context(|| format!("generating `{name}`"))?;
    if args.dry_run {
        for file in &files {
            println!("{}", args.out.join(&file.path).display());
        }
        println!(
            "{name}: {} file(s) -> {} (dry run, nothing written)",
            files.len(),
            args.out.display()
        );
        return Ok(());
    }
    write(&args.out, &files).with_context(|| format!("generating `{name}`"))?;
    println!("{name}: {} file(s) -> {}", files.len(), args.out.display());

    Ok(())
}

/// Expand the paths given on the command line: files are taken as they are,
/// directories are searched recursively for `*.uju`. A path reached twice —
/// listed twice, or listed alongside a directory containing it — is compiled
/// once, and each directory is listed in sorted order, so the same inputs
/// always lower to the same IR.
fn collect(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let metadata =
            fs::metadata(path).with_context(|| format!("reading `{}`", path.display()))?;
        if metadata.is_dir() {
            walk(path, &mut files, &mut seen)?;
        } else {
            push(path.clone(), &mut files, &mut seen)?;
        }
    }
    Ok(files)
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>) -> Result<()> {
    // Directories are marked too, so a symlink cycle terminates.
    if !mark(dir, seen)? {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect()
        })
        .with_context(|| format!("reading `{}`", dir.display()))?;
    entries.sort();

    for entry in entries {
        if entry.is_dir() {
            walk(&entry, files, seen)?;
        } else if entry.extension() == Some(OsStr::new(EXTENSION)) {
            push(entry, files, seen)?;
        }
    }
    Ok(())
}

fn push(path: PathBuf, files: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>) -> Result<()> {
    if mark(&path, seen)? {
        files.push(path);
    }
    Ok(())
}

/// Records `path` as visited, returning whether it had not been seen before.
fn mark(path: &Path, seen: &mut BTreeSet<PathBuf>) -> Result<bool> {
    let canonical =
        fs::canonicalize(path).with_context(|| format!("resolving `{}`", path.display()))?;
    Ok(seen.insert(canonical))
}

fn write(dir: &Path, files: &[GeneratedFile]) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating `{}`", dir.display()))?;
    for file in files {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating `{}`", parent.display()))?;
        }
        fs::write(&path, &file.contents)
            .with_context(|| format!("writing `{}`", path.display()))?;
    }
    Ok(())
}
