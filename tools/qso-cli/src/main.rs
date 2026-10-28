use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use grayline_qso::{Directory, Origin, Store, default_store_path, import};
use grayline_qso_cli::{record_from_assignments, render_json, render_keys, render_table};

/// Exit status for a lookup that found nothing. Clap reports a misused command
/// line as 2, so a station nobody knows about cannot claim that code.
const NOT_FOUND_EXIT_CODE: u8 = 3;

/// The contact directory the Grayline applications read.
#[derive(Debug, Parser)]
#[command(name = "gl-qso", version, about, long_about = None)]
struct Cli {
    /// Contact store to work on.
    #[arg(long, global = true, value_name = "PATH")]
    store: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Prints where the store is.
    Path,
    /// Prints the keys an import fills in.
    Keys,
    /// Prints what is filed under a callsign.
    Lookup(LookupArgs),
    /// Files values by hand, which no import will overwrite.
    Set(SetArgs),
    /// Drops fields from a station.
    Unset(UnsetArgs),
    /// Forgets a station outright.
    Remove(RemoveArgs),
    /// Prints the callsigns the store holds.
    List(ListArgs),
    /// Reads ADIF logs into the store.
    Import(ImportArgs),
}

#[derive(Debug, Args)]
struct LookupArgs {
    /// Station to look up.
    #[arg(value_name = "CALLSIGN")]
    callsign: String,

    /// How to print the record.
    #[arg(long, value_name = "FORMAT", default_value = "table")]
    format: Format,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Format {
    Table,
    Json,
}

#[derive(Debug, Args)]
struct SetArgs {
    #[arg(value_name = "CALLSIGN")]
    callsign: String,

    /// Fields to file, as `key=value`.
    #[arg(value_name = "KEY=VALUE", required = true)]
    assignments: Vec<String>,
}

#[derive(Debug, Args)]
struct UnsetArgs {
    #[arg(value_name = "CALLSIGN")]
    callsign: String,

    #[arg(value_name = "KEY", required = true)]
    keys: Vec<String>,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    #[arg(value_name = "CALLSIGN")]
    callsign: String,

    /// Do not ask first.
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct ListArgs {
    /// Only callsigns starting with this.
    #[arg(long, value_name = "TEXT", default_value = "")]
    prefix: String,

    /// How many to print at most.
    #[arg(long, value_name = "N", default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// ADIF logs to read.
    #[arg(value_name = "FILE.adi", required = true)]
    files: Vec<PathBuf>,

    /// The encoding the logs were written in. Turbo HAMLOG and the loggers of
    /// its generation write `shift_jis`.
    #[arg(long, value_name = "ENCODING", default_value = "utf-8")]
    encoding: String,

    /// Report what would be written and write nothing.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("gl-qso: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Path => path(cli.store.as_deref()),
        Command::Keys => {
            print!("{}", render_keys());
            Ok(ExitCode::SUCCESS)
        }
        Command::Lookup(args) => lookup(open(cli.store)?, args),
        Command::Set(args) => set(open(cli.store)?, args),
        Command::Unset(args) => unset(open(cli.store)?, args),
        Command::Remove(args) => remove(open(cli.store)?, args),
        Command::List(args) => list(&open(cli.store)?, args),
        Command::Import(args) => import_logs(open(cli.store)?, args),
    }
}

fn open(named: Option<PathBuf>) -> Result<Store> {
    let path = match named {
        Some(path) => path,
        None => default_store_path().context("this platform has no user directories to keep a store in")?,
    };
    Store::open(&path).map_err(Into::into)
}

fn path(named: Option<&std::path::Path>) -> Result<ExitCode> {
    match named.map(PathBuf::from).or_else(default_store_path) {
        Some(path) => {
            let state = if path.exists() { "present" } else { "not written yet" };
            println!("{} ({state})", path.display());
        }
        None => println!("this platform has no user directory for the store"),
    }
    Ok(ExitCode::SUCCESS)
}

fn lookup(store: Store, args: LookupArgs) -> Result<ExitCode> {
    let answer = Directory::new(store).look_up(&args.callsign)?;
    if !answer.known {
        eprintln!("gl-qso: nothing is filed under {}", answer.record.callsign());
        return Ok(ExitCode::from(NOT_FOUND_EXIT_CODE));
    }
    match args.format {
        Format::Table => print!("{}", render_table(&answer.record)),
        Format::Json => println!("{}", render_json(&answer.record)),
    }
    Ok(ExitCode::SUCCESS)
}

fn set(mut store: Store, args: SetArgs) -> Result<ExitCode> {
    let record = record_from_assignments(&args.callsign, &args.assignments).map_err(anyhow::Error::msg)?;
    let written = store.merge(&record, Origin::Manual)?;
    println!("{written} field(s) filed under {}", record.callsign());
    Ok(ExitCode::SUCCESS)
}

fn unset(mut store: Store, args: UnsetArgs) -> Result<ExitCode> {
    if store.get(&args.callsign)?.is_none() {
        bail!("nothing is filed under {}", args.callsign);
    }
    let mut dropped = 0;
    for key in &args.keys {
        if store.unset(&args.callsign, &key.to_ascii_lowercase())? {
            dropped += 1;
        }
    }
    println!("{dropped} field(s) dropped from {}", args.callsign.to_ascii_uppercase());
    Ok(ExitCode::SUCCESS)
}

fn remove(mut store: Store, args: RemoveArgs) -> Result<ExitCode> {
    if !args.yes && !confirm(&format!("Forget everything filed under {}?", args.callsign))? {
        println!("nothing was removed");
        return Ok(ExitCode::SUCCESS);
    }
    if store.remove(&args.callsign)? {
        println!("{} was forgotten", args.callsign.to_ascii_uppercase());
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("gl-qso: nothing is filed under {}", args.callsign);
        Ok(ExitCode::from(NOT_FOUND_EXIT_CODE))
    }
}

fn list(store: &Store, args: ListArgs) -> Result<ExitCode> {
    let callsigns = store.list(&args.prefix, args.limit)?;
    for callsign in &callsigns {
        println!("{callsign}");
    }
    if callsigns.len() == args.limit {
        eprintln!("gl-qso: stopped at {}; pass --limit for more", args.limit);
    }
    Ok(ExitCode::SUCCESS)
}

fn import_logs(mut store: Store, args: ImportArgs) -> Result<ExitCode> {
    for file in &args.files {
        let source = fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;
        // A dry run reads into a store of its own rather than counting what a
        // real one would have written: the counting rule is the store's, and
        // reimplementing it here is how the report and the write come to
        // disagree.
        let mut target = if args.dry_run {
            Store::in_memory()?
        } else {
            std::mem::replace(&mut store, Store::in_memory()?)
        };
        let report = import(&mut target, &source, &args.encoding)?;
        if !args.dry_run {
            store = target;
        }

        let verb = if args.dry_run { "would file" } else { "filed" };
        println!(
            "{}: {} QSOs read, {verb} {} field(s) for {} station(s)",
            file.display(),
            report.read,
            report.fields,
            report.stations
        );
        for skipped in &report.skipped {
            eprintln!("gl-qso: {skipped}");
        }
    }
    store.checkpoint()?;
    Ok(ExitCode::SUCCESS)
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    io::stdout().flush().context("failed to ask")?;
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("failed to read the answer")?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }
}
