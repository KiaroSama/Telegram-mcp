//! CI evidence and publication guards. No command prints credentials or source snippets.
#[path = "ci/coverage.rs"]
mod coverage;
#[path = "ci/evidence.rs"]
mod evidence;
#[path = "ci/gitleaks.rs"]
mod gitleaks;
#[path = "ci/history.rs"]
mod history;
#[path = "ci/privacy.rs"]
mod privacy;
#[path = "ci/process.rs"]
mod process;
#[path = "ci/status.rs"]
mod status;

use anyhow::{Result, bail};
use std::{env, path::Path};

fn main() {
    if env::var_os("GITLEAKS_REAL_GIT").is_some()
        && env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_owned()))
            .is_some_and(|n| n == "git" || n == "git.exe")
    {
        std::process::exit(gitleaks::git_child());
    }
    if let Err(error) = run() {
        // Errors are written by this helper, never a subprocess's captured output.
        eprintln!("CI evidence refused: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("record") if args.len() == 4 => {
            evidence::record(&args[1], Path::new(&args[2]), Path::new(&args[3]))
        }
        Some("compare") if args.len() == 2 => evidence::compare(Path::new(&args[1])),
        Some("coverage") if args.len() == 2 => coverage::run(Path::new(&args[1])),
        Some("parity") if args.len() == 2 => evidence::parity(Path::new(&args[1])),
        Some("privacy") if args.len() == 1 => privacy::scan(),
        Some("privacy-history") if args.len() == 1 => history::scan(),
        Some("gitleaks") if args.len() == 1 => gitleaks::scan(),
        Some("gitleaks-full-history") if args.len() == 1 => gitleaks::scan_full_history(),
        Some("status") if args.len() == 3 => status::publish(&args[1], &args[2]),
        Some("package") if args.len() == 2 => evidence::package(&args[1]),
        Some("--help") | Some("help") | None => {
            println!(
                "ci record LABEL JUNIT OUTPUT\nci compare RECORDS_DIR\nci coverage LCOV_DIR\nci parity RECORDS_DIR\nci privacy\nci gitleaks\nci status begin|finish WORKFLOW\nci package TARGET"
            );
            Ok(())
        }
        _ => bail!("Invalid CI command; use ci --help"),
    }
}
