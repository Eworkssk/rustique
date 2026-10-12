//! `artisan`-style commands for an application's own binary.
//!
//! Migrations are compiled into the application, so the commands run from a
//! small binary of its own, say `src/bin/rustique.rs`:
//!
//! ```no_run
//! # mod migrations { pub fn all() -> Vec<rustique::Migration> { Vec::new() } }
//! #[tokio::main]
//! async fn main() -> rustique::Result<()> {
//!     let migrator = rustique::Migrator::new(migrations::all());
//!     rustique::cli::run(&migrator, std::env::args().skip(1)).await
//! }
//! ```
//!
//! Then `cargo run --bin rustique -- migrate`. Commands:
//!
//! - `migrate [--seed]`, `migrate:rollback [--step <n>]`, `migrate:reset --force`,
//!   `migrate:fresh --force [--seed]`, `migrate:status`, `db:seed`: as [`Migrator`]'s
//!   methods ([`rollback_steps`](Migrator::rollback_steps) for `--step`,
//!   [`seed`](Migrator::seed) for `--seed` and `db:seed`), on the database
//!   whose URL is in `$DATABASE_URL`, or in the variable named by
//!   `--env <VAR>`. The URL holds a password, so it is never a command-line
//!   argument, where `ps` and shell history show it. `migrate:reset` and
//!   `migrate:fresh` wipe the database, so they refuse to run without `--force`.
//! - `make:migration <name> [--path <dir>]`: writes a migration module to
//!   `<dir>` (default `src/migrations`), named like
//!   `m2026_10_11_093000_create_users_table.rs`; a `create_<table>_table`
//!   name gets a `Schema::create` skeleton. Register it in
//!   [`migrations!`](crate::migrations) yourself.
//! - `schema:snapshot [--path <file>]`: writes the columns of every table to
//!   `<file>` (default `rustique-schema.txt`), for the compile-time check of
//!   `#[rustique::model]` structs (cargo feature `schema-check`). Run it from the
//!   directory of the crate that defines the models, after migrating.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use chrono::Utc;

use crate::{Db, Error, Migrator, Result};

const USAGE: &str = "usage: <command> [--env <VAR holding the database URL; default DATABASE_URL>]
commands:
  migrate [--seed]                         run pending migrations
  migrate:rollback [--step <n>]            roll back the last batch, or the last n migrations
  migrate:reset --force                    roll back every migration
  migrate:fresh --force [--seed]           drop every table, then run every migration
  migrate:status                           list migrations and their batch
  db:seed                                  run the seeder
  make:migration <name> [--path <dir>]     create a migration file
  schema:snapshot [--path <file>]          write the schema snapshot for the compile-time model check";

/// Runs the command in `args` (without the program name) and prints its
/// output to stdout.
///
/// # Errors
///
/// [`Error::Config`] for an unknown command, bad arguments, an unset URL
/// variable or a file that cannot be written; otherwise the
/// [`Migrator`]'s errors.
pub async fn run(migrator: &Migrator, args: impl IntoIterator<Item = String>) -> Result<()> {
    for line in execute(migrator, args).await? {
        println!("{line}");
    }
    Ok(())
}

/// [`run`], returning the output lines.
async fn execute(
    migrator: &Migrator,
    args: impl IntoIterator<Item = String>,
) -> Result<Vec<String>> {
    let args = Args::parse(args)?;
    if matches!(args.command.as_str(), "migrate:fresh" | "migrate:reset") && !args.force {
        return Err(usage(&format!(
            "{} wipes the database; add --force to run it",
            args.command
        )));
    }
    let lines = match args.command.as_str() {
        "migrate" | "migrate:fresh" => {
            let db = args.connect().await?;
            let done = if args.command == "migrate" {
                migrator.migrate(&db).await?
            } else {
                migrator.fresh(&db).await?
            };
            let mut lines = report(done, "Migrated", "Nothing to migrate.");
            if args.seed {
                migrator.seed(&db).await?;
                lines.push("Seeded.".to_owned());
            }
            lines
        }
        "migrate:rollback" => {
            let db = args.connect().await?;
            let done = match &args.step {
                Some(step) => {
                    let steps = step
                        .parse()
                        .map_err(|_| usage(&format!("--step takes a number, not `{step}`")))?;
                    migrator.rollback_steps(&db, steps).await?
                }
                None => migrator.rollback(&db).await?,
            };
            report(done, "Rolled back", "Nothing to roll back.")
        }
        "migrate:reset" => {
            let done = migrator.reset(&args.connect().await?).await?;
            report(done, "Rolled back", "Nothing to roll back.")
        }
        "db:seed" => {
            migrator.seed(&args.connect().await?).await?;
            vec!["Seeded.".to_owned()]
        }
        "migrate:status" => {
            let status = migrator.status(&args.connect().await?).await?;
            status
                .iter()
                .map(|m| match m.batch {
                    Some(batch) => format!("Ran [{batch}]  {}", m.name),
                    None => format!("Pending    {}", m.name),
                })
                .collect()
        }
        "make:migration" => {
            let [name] = &args.positional[..] else {
                return Err(usage("make:migration takes one name"));
            };
            let dir = args.path.as_deref().unwrap_or("src/migrations");
            make_migration(Path::new(dir), name)?
        }
        "schema:snapshot" => {
            let text = crate::schema::snapshot(&args.connect().await?).await?;
            let file = args.path.as_deref().unwrap_or(crate::schema::SNAPSHOT_FILE);
            fs::write(file, text)
                .map_err(|e| Error::Config(format!("cannot write {file}: {e}")))?;
            vec![format!("Wrote: {file}")]
        }
        "" | "help" | "list" => USAGE.lines().map(str::to_owned).collect(),
        other => return Err(usage(&format!("unknown command `{other}`"))),
    };
    Ok(lines)
}

fn usage(problem: &str) -> Error {
    Error::Config(format!("{problem}\n{USAGE}"))
}

fn report(names: Vec<&str>, done: &str, nothing: &str) -> Vec<String> {
    if names.is_empty() {
        return vec![nothing.to_owned()];
    }
    names.iter().map(|name| format!("{done}: {name}")).collect()
}

#[derive(Debug, Default)]
struct Args {
    command: String,
    positional: Vec<String>,
    env: Option<String>,
    path: Option<String>,
    step: Option<String>,
    seed: bool,
    force: bool,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| usage(&format!("{arg} needs a value")))
            };
            match arg.as_str() {
                "--env" => parsed.env = Some(value()?),
                "--path" => parsed.path = Some(value()?),
                "--step" => parsed.step = Some(value()?),
                "--seed" => parsed.seed = true,
                "--force" => parsed.force = true,
                flag if flag.starts_with("--") => {
                    return Err(usage(&format!("unknown option `{flag}`")));
                }
                _ if parsed.command.is_empty() => parsed.command = arg,
                _ => parsed.positional.push(arg),
            }
        }
        Ok(parsed)
    }

    async fn connect(&self) -> Result<Db> {
        let var = self.env.as_deref().unwrap_or("DATABASE_URL");
        let url =
            std::env::var(var).map_err(|_| usage(&format!("set {var} to the database URL")))?;
        Db::connect(&url).await
    }
}

/// Writes a new migration module into `dir`.
fn make_migration(dir: &Path, name: &str) -> Result<Vec<String>> {
    let valid = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(usage(&format!(
            "migration name `{name}` must be snake_case: create_users_table"
        )));
    }
    let module = format!("m{}_{name}", Utc::now().format("%Y_%m_%d_%H%M%S"));
    let file = dir.join(format!("{module}.rs"));
    let io = |e: std::io::Error| Error::Config(format!("cannot write {}: {e}", file.display()));
    fs::create_dir_all(dir).map_err(io)?;
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .map_err(io)?;
    out.write_all(template(name).as_bytes()).map_err(io)?;
    Ok(vec![
        format!("Created: {}", file.display()),
        format!("Register it: `mod {module};` and `{module}` in `rustique::migrations![...]`"),
    ])
}

fn template(name: &str) -> String {
    let table = name
        .strip_prefix("create_")
        .and_then(|rest| rest.strip_suffix("_table"))
        .filter(|table| !table.is_empty());
    match table {
        Some(table) => format!(
            r#"use rustique::{{Db, Schema}};

pub async fn up(db: &Db) -> rustique::Result<()> {{
    Schema::create(db, "{table}", |t| {{
        t.id();
        t.timestamps();
    }})
    .await
}}

pub async fn down(db: &Db) -> rustique::Result<()> {{
    Schema::drop_if_exists(db, "{table}").await
}}
"#
        ),
        None => r#"use rustique::Db;

pub async fn up(db: &Db) -> rustique::Result<()> {
    let _ = db; // rustique::Schema::table(db, "users", |t| { ... }).await
    Ok(())
}

pub async fn down(db: &Db) -> rustique::Result<()> {
    let _ = db;
    Ok(())
}
"#
        .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    async fn execute_line(line: &str) -> Result<Vec<String>> {
        execute(&Migrator::new(Vec::new()), args(line)).await
    }

    #[tokio::test]
    async fn makes_migrations() {
        let dir = std::env::temp_dir().join(format!("rustique-cli-{}", std::process::id()));
        let path = dir.display().to_string();

        let lines = execute_line(&format!("make:migration create_users_table --path {path}"))
            .await
            .unwrap();
        let created = lines[0].strip_prefix("Created: ").unwrap();
        assert!(created.ends_with("_create_users_table.rs"), "{created}");
        let source = fs::read_to_string(created).unwrap();
        assert!(source.contains(r#"Schema::create(db, "users""#), "{source}");
        assert!(
            source.contains(r#"drop_if_exists(db, "users")"#),
            "{source}"
        );

        let lines = execute_line(&format!("make:migration add_votes --path {path}"))
            .await
            .unwrap();
        let source = fs::read_to_string(lines[0].strip_prefix("Created: ").unwrap()).unwrap();
        assert!(source.contains("pub async fn down"), "{source}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn rejects_bad_arguments() {
        for line in [
            "make:migration",
            "make:migration Bad-Name",
            "make:migration a b",
            "frobnicate",
            "migrate --verbose",
            "migrate --env",
            "migrate --database mysql://u:secret@h/db",
            "migrate --env ORM_CLI_TEST_UNSET_VARIABLE",
            "migrate:rollback --step",
            "migrate:fresh",
            "migrate:reset --seed",
        ] {
            let error = execute_line(line).await.unwrap_err();
            assert!(matches!(error, Error::Config(_)), "{line}: {error}");
        }
        let error = execute_line("migrate --env PATH").await.unwrap_err();
        assert!(
            error.to_string().contains("unsupported URL scheme"),
            "{error}"
        );
        assert!(execute_line("").await.unwrap()[0].starts_with("usage"));

        // Without `--force` the wiping commands stop before connecting.
        let error = execute_line("migrate:fresh").await.unwrap_err();
        assert!(error.to_string().contains("--force"), "{error}");
        let error = execute_line("migrate:reset --force --env ORM_CLI_TEST_UNSET_VARIABLE")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("set ORM_CLI_TEST_UNSET_VARIABLE"),
            "{error}"
        );
    }
}
