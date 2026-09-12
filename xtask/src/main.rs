#![allow(
    clippy::disallowed_macros,
    reason = "the command-line task runner writes user-facing status"
)]

use std::{
    env,
    process::{Command, exit},
};

const HELP: &str =
    "Usage: cargo xtask benchmark <check|criterion|deterministic|allocations|process|heap|all>\n";
fn run(args: &[&str]) -> Result<(), String> {
    let status = Command::new("cargo").args(args).status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("cargo {args:?} failed: {status}")) }
}
fn script(name: &str) -> Result<(), String> {
    let status = Command::new(format!("scripts/{name}")).status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("{name} failed: {status}")) }
}
fn needs(command: &str) -> Result<(), String> {
    if Command::new("sh")
        .args(["-c", &format!("command -v {command} >/dev/null")])
        .status()
        .map_err(|e| e.to_string())?
        .success()
    {
        Ok(())
    } else {
        Err(format!(
            "required tool unavailable: {command}; install it locally or run `cargo xtask benchmark check` for compile-only validation"
        ))
    }
}
fn benchmark(kind: &str) -> Result<(), String> {
    match kind {
        "check" => {
            for bench in ["criterion_browser", "gungraun_sync", "allocation_sync"] {
                run(&["bench", "-p", "voidcrawl-benchmarks", "--bench", bench, "--no-run"])?;
            }
            run(&["build", "--release", "-p", "voidcrawl-benchmarks", "--bin", "profile_browser"])
        }
        "criterion" => script("run-cas-317-criterion.sh"),
        "deterministic" => {
            needs("valgrind")?;
            needs("gungraun-runner")?;
            script("run-cas-317-deterministic.sh")
        }
        "allocations" => script("run-cas-317-allocations.sh"),
        "process" => {
            needs("/usr/bin/time")?;
            script("run-cas-317-process.sh")
        }
        "heap" => {
            needs("valgrind")?;
            script("run-cas-317-heap.sh")
        }
        "all" => {
            for k in ["criterion", "deterministic", "allocations", "process", "heap"] {
                benchmark(k)?;
            }
            Ok(())
        }
        _ => Err(HELP.into()),
    }
}
fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args.as_slice() {
        [a, b] if a == "benchmark" => benchmark(b.as_str()),
        [a] if a == "help" || a == "--help" => {
            print!("{HELP}");
            Ok(())
        }
        _ => Err(HELP.into()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        exit(1)
    }
}
