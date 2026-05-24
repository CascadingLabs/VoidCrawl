//! `voidcrawl-mcp install` — wire the server into Claude Code, Codex, and
//! opencode without hand-editing config.
//!
//! Hybrid strategy: delegate to a host's own `mcp add` CLI where one exists
//! and is scriptable (Claude, Codex at user scope), write the host's config
//! file directly where the CLI can't help (opencode's `mcp add` is
//! interactive; Codex has no project scope), and when a host isn't installed
//! at all, print the exact block to paste in once it is.
//!
//! Every wiring points at the absolute path of the running binary
//! (`env::current_exe`), so it never depends on the host inheriting our
//! `PATH` — the failure mode that makes hand-wired configs flaky.

use std::{
    env,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result};
use clap::{Args, ValueEnum};
use serde_json::{Map, Value};

/// Server key written into every host config.
const SERVER_NAME: &str = "voidcrawl";

/// Pool sizing the wired server launches with — mirrors the documented
/// defaults. Single source of truth for both the CLI `--env` flags and the
/// hand-edit blocks we print.
const SERVER_ENV: &[(&str, &str)] =
    &[("BROWSER_COUNT", "1"), ("TABS_PER_BROWSER", "5"), ("CHROME_HEADLESS", "1")];

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// Your personal config — works in every repo.
    User,
    /// Committed, in-repo config for this project.
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Host {
    Claude,
    Codex,
    Opencode,
}

impl Host {
    const ALL: [Host; 3] = [Host::Claude, Host::Codex, Host::Opencode];
}

/// Flags shared by the `install` and `uninstall` subcommands.
#[derive(Debug, Clone, Args)]
pub struct InstallArgs {
    /// Config scope to write.
    #[arg(long, value_enum, default_value_t = Scope::User)]
    pub scope:   Scope,
    /// Host to target; repeat to pick several. Defaults to all three.
    #[arg(long, value_enum)]
    pub tool:    Vec<Host>,
    /// Print what would change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Report where the server is already wired, instead of writing (install
    /// only).
    #[arg(long)]
    pub status:  bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Install,
    Uninstall,
    Status,
}

/// Resolved request after folding the subcommand and flags together.
#[derive(Debug, Clone)]
struct Options {
    action:  Action,
    scope:   Scope,
    hosts:   Vec<Host>,
    dry_run: bool,
}

/// Entry point from `main`: `uninstall` picks the verb; `--status` (install
/// only) reports state instead of writing. clap has already validated args.
pub fn run(uninstall: bool, args: &InstallArgs) -> Result<()> {
    let action = if uninstall {
        Action::Uninstall
    } else if args.status {
        Action::Status
    } else {
        Action::Install
    };
    let hosts = if args.tool.is_empty() { Host::ALL.to_vec() } else { args.tool.clone() };
    dispatch(&Options { action, scope: args.scope, hosts, dry_run: args.dry_run })
}

fn dispatch(opts: &Options) -> Result<()> {
    let exe = env::current_exe().context("resolving the voidcrawl-mcp binary path")?;
    let exe = exe.to_string_lossy().into_owned();
    let mut out = io::stdout().lock();

    if opts.action == Action::Status {
        return status(&mut out, opts);
    }
    for &host in &opts.hosts {
        match host {
            Host::Claude => claude(&mut out, opts, &exe)?,
            Host::Codex => codex(&mut out, opts, &exe)?,
            Host::Opencode => opencode(&mut out, opts, &exe)?,
        }
    }
    Ok(())
}

fn scope_str(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project => "project",
    }
}

// ---- Claude Code -------------------------------------------------------

fn claude(out: &mut impl io::Write, opts: &Options, exe: &str) -> Result<()> {
    let label = "Claude Code";
    if !on_path("claude") {
        if opts.action == Action::Uninstall {
            writeln!(out, "[{label}] CLI not found; nothing to remove")?;
            return Ok(());
        }
        let hint = ".mcp.json (project) or ~/.claude.json (user)";
        return print_manual(out, label, &missing_lead(hint), &claude_manual_json(exe)?);
    }
    let argv = match opts.action {
        Action::Install => claude_add_argv(opts.scope, exe),
        Action::Uninstall => vec![
            "mcp".into(),
            "remove".into(),
            "--scope".into(),
            scope_str(opts.scope).into(),
            SERVER_NAME.into(),
        ],
        Action::Status => return Ok(()),
    };
    run_cli(out, label, opts.dry_run, "claude", &argv)
}

fn claude_add_argv(scope: Scope, exe: &str) -> Vec<String> {
    let mut argv = vec![
        "mcp".into(),
        "add".into(),
        "--scope".into(),
        scope_str(scope).into(),
        "--transport".into(),
        "stdio".into(),
    ];
    for (k, v) in SERVER_ENV {
        argv.push("--env".into());
        argv.push(format!("{k}={v}"));
    }
    argv.push(SERVER_NAME.into());
    argv.push("--".into());
    argv.push(exe.into());
    argv
}

fn claude_manual_json(exe: &str) -> Result<String> {
    let entry = server_entry_common(exe);
    let block = wrap("mcpServers", wrap(SERVER_NAME, entry));
    Ok(serde_json::to_string_pretty(&block)?)
}

/// `{ "command": <exe>, "env": { … } }` — the shape Claude Code and the
/// hand-edit block share.
fn server_entry_common(exe: &str) -> Value {
    wrap_pair("command", Value::String(exe.to_owned()), "env", env_object())
}

// ---- Codex -------------------------------------------------------------

fn codex(out: &mut impl io::Write, opts: &Options, exe: &str) -> Result<()> {
    let label = "Codex";
    // The Codex CLI only writes the global config, so a committed
    // project-scoped server can only be done by hand.
    if opts.scope == Scope::Project {
        if opts.action == Action::Uninstall {
            writeln!(
                out,
                "[{label}] remove the [mcp_servers.{SERVER_NAME}] block from ./.codex/config.toml"
            )?;
        } else {
            let lead = "the Codex CLI only writes global config — add this to \
                        ./.codex/config.toml for a project-scoped server:";
            print_manual(out, label, lead, &codex_manual_toml(exe))?;
        }
        return Ok(());
    }
    if !on_path("codex") {
        if opts.action == Action::Uninstall {
            writeln!(out, "[{label}] CLI not found; nothing to remove")?;
            return Ok(());
        }
        return print_manual(
            out,
            label,
            &missing_lead("~/.codex/config.toml"),
            &codex_manual_toml(exe),
        );
    }
    let argv = match opts.action {
        Action::Install => codex_add_argv(exe),
        Action::Uninstall => vec!["mcp".into(), "remove".into(), SERVER_NAME.into()],
        Action::Status => return Ok(()),
    };
    run_cli(out, label, opts.dry_run, "codex", &argv)
}

fn codex_add_argv(exe: &str) -> Vec<String> {
    let mut argv = vec!["mcp".into(), "add".into()];
    for (k, v) in SERVER_ENV {
        argv.push("--env".into());
        argv.push(format!("{k}={v}"));
    }
    argv.push(SERVER_NAME.into());
    argv.push("--".into());
    argv.push(exe.into());
    argv
}

fn codex_manual_toml(exe: &str) -> String {
    let env_lines = SERVER_ENV.iter().fold(String::new(), |mut acc, (k, v)| {
        // Writing to a String is infallible; the Result is only there to
        // satisfy the `fmt::Write` signature.
        let _ = writeln!(acc, "{k} = \"{v}\"");
        acc
    });
    format!(
        "[mcp_servers.{SERVER_NAME}]\ncommand = \"{exe}\"\nargs = []\n\n\
         [mcp_servers.{SERVER_NAME}.env]\n{env_lines}"
    )
}

// ---- opencode ----------------------------------------------------------

fn opencode(out: &mut impl io::Write, opts: &Options, exe: &str) -> Result<()> {
    let label = "opencode";
    if !on_path("opencode") {
        if opts.action == Action::Uninstall {
            writeln!(out, "[{label}] not found; nothing to remove")?;
            return Ok(());
        }
        let hint = "opencode.json (project) or ~/.config/opencode/opencode.json (user)";
        return print_manual(out, label, &missing_lead(hint), &opencode_manual_json(exe)?);
    }

    let path = opencode_path(opts.scope)?;
    let existing = read_opt(&path)?;
    let value = match opts.action {
        Action::Uninstall => {
            let Some(text) = existing.as_deref() else {
                writeln!(out, "[{label}] no {} to edit", path.display())?;
                return Ok(());
            };
            opencode_remove(text)?
        }
        _ => opencode_merge(existing.as_deref(), exe)?,
    };
    let verb = if opts.action == Action::Uninstall { "updated" } else { "wired in" };
    commit_json(out, opts.dry_run, label, &path, &value, existing.is_some(), verb)
}

fn opencode_path(scope: Scope) -> Result<PathBuf> {
    match scope {
        Scope::Project => {
            Ok(env::current_dir().context("resolving the current directory")?.join("opencode.json"))
        }
        Scope::User => Ok(config_home()
            .context("resolving XDG config dir / HOME for opencode")?
            .join("opencode")
            .join("opencode.json")),
    }
}

/// Merge the voidcrawl entry into an existing opencode config (or a fresh
/// one), preserving every other key and MCP server.
fn opencode_merge(existing: Option<&str>, exe: &str) -> Result<Value> {
    let mut root = match existing {
        Some(s) if !s.trim().is_empty() => {
            serde_json::from_str::<Value>(s).context("parsing existing opencode.json")?
        }
        _ => Value::Object(Map::new()),
    };
    let obj = root.as_object_mut().context("opencode.json root is not a JSON object")?;
    let mcp = obj.entry("mcp").or_insert_with(|| Value::Object(Map::new()));
    let mcp_obj = mcp.as_object_mut().context("opencode.json `mcp` is not a JSON object")?;
    mcp_obj.insert(SERVER_NAME.to_owned(), opencode_entry(exe));
    Ok(root)
}

fn opencode_remove(text: &str) -> Result<Value> {
    let mut root: Value = serde_json::from_str(text).context("parsing opencode.json")?;
    if let Some(mcp) = root.get_mut("mcp").and_then(Value::as_object_mut) {
        mcp.remove(SERVER_NAME);
    }
    Ok(root)
}

fn opencode_entry(exe: &str) -> Value {
    let mut m = Map::new();
    m.insert("type".to_owned(), Value::String("local".to_owned()));
    m.insert("command".to_owned(), Value::Array(vec![Value::String(exe.to_owned())]));
    m.insert("enabled".to_owned(), Value::Bool(true));
    m.insert("environment".to_owned(), env_object());
    Value::Object(m)
}

fn opencode_manual_json(exe: &str) -> Result<String> {
    let block = wrap("mcp", wrap(SERVER_NAME, opencode_entry(exe)));
    Ok(serde_json::to_string_pretty(&block)?)
}

// ---- status ------------------------------------------------------------

fn status(out: &mut impl io::Write, opts: &Options) -> Result<()> {
    for &host in &opts.hosts {
        match host {
            Host::Claude => status_cli(out, "Claude Code", "claude")?,
            Host::Codex => status_cli(out, "Codex", "codex")?,
            Host::Opencode => status_opencode(out, opts.scope)?,
        }
    }
    Ok(())
}

fn status_cli(out: &mut impl io::Write, label: &str, prog: &str) -> Result<()> {
    if !on_path(prog) {
        writeln!(out, "[{label}] CLI not found")?;
        return Ok(());
    }
    let configured = Command::new(prog)
        .args(["mcp", "get", SERVER_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    writeln!(out, "[{label}] {}", if configured { "configured" } else { "not configured" })?;
    Ok(())
}

fn status_opencode(out: &mut impl io::Write, scope: Scope) -> Result<()> {
    let path = opencode_path(scope)?;
    let configured = read_opt(&path)?
        .as_deref()
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| v.get("mcp").and_then(|m| m.get(SERVER_NAME)).map(|_| true))
        .unwrap_or(false);
    writeln!(
        out,
        "[opencode] {} ({})",
        if configured { "configured" } else { "not configured" },
        path.display()
    )?;
    Ok(())
}

// ---- shared helpers ----------------------------------------------------

/// Is `bin` on `PATH`? We search rather than spawn so detection has no side
/// effects and stays fast.
fn on_path(bin: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
}

fn run_cli(
    out: &mut impl io::Write,
    label: &str,
    dry_run: bool,
    prog: &str,
    argv: &[String],
) -> Result<()> {
    if dry_run {
        writeln!(out, "[{label}] would run: {prog} {}", argv.join(" "))?;
        return Ok(());
    }
    let status = Command::new(prog)
        .args(argv)
        .status()
        .with_context(|| format!("running `{prog}` for {label}"))?;
    if status.success() {
        writeln!(out, "[{label}] wired via `{prog} mcp`")?;
    } else {
        writeln!(out, "[{label}] `{prog} mcp` exited with {status}")?;
    }
    Ok(())
}

fn print_manual(out: &mut impl io::Write, label: &str, lead: &str, block: &str) -> Result<()> {
    writeln!(out, "[{label}] {lead}\n\n{block}\n")?;
    Ok(())
}

/// Lead line for the common case: the host's CLI isn't installed, so we hand
/// the user the block to paste once it is.
fn missing_lead(hint: &str) -> String {
    format!("CLI not found on PATH — add this to {hint} once it's installed:")
}

fn commit_json(
    out: &mut impl io::Write,
    dry_run: bool,
    label: &str,
    path: &Path,
    value: &Value,
    had_existing: bool,
    verb: &str,
) -> Result<()> {
    let text = format!("{}\n", serde_json::to_string_pretty(value)?);
    if dry_run {
        writeln!(out, "[{label}] would write {}:\n{text}", path.display())?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    if had_existing {
        let bak = PathBuf::from(format!("{}.bak", path.display()));
        if let Err(e) = fs::copy(path, &bak) {
            writeln!(out, "[{label}] warning: backup to {} failed: {e}", bak.display())?;
        }
    }
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    writeln!(out, "[{label}] {verb} {}", path.display())?;
    Ok(())
}

fn read_opt(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// `$XDG_CONFIG_HOME` (if absolute) else `~/.config`. Mirrors how the core
/// crate resolves Chrome's config root.
fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn env_object() -> Value {
    let mut m = Map::new();
    for (k, v) in SERVER_ENV {
        m.insert((*k).to_owned(), Value::String((*v).to_owned()));
    }
    Value::Object(m)
}

fn wrap(key: &str, val: Value) -> Value {
    let mut m = Map::new();
    m.insert(key.to_owned(), val);
    Value::Object(m)
}

fn wrap_pair(k1: &str, v1: Value, k2: &str, v2: Value) -> Value {
    let mut m = Map::new();
    m.insert(k1.to_owned(), v1);
    m.insert(k2.to_owned(), v2);
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test harness")]

    use clap::Parser;

    use super::*;

    // Mirrors how `main` mounts these as subcommands, so the tests exercise
    // clap exactly as the binary does.
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: TestCmd,
    }

    #[derive(clap::Subcommand)]
    enum TestCmd {
        Install(InstallArgs),
        Uninstall(InstallArgs),
    }

    fn args_of(argv: &[&str]) -> InstallArgs {
        match TestCli::parse_from(argv).cmd {
            TestCmd::Install(a) | TestCmd::Uninstall(a) => a,
        }
    }

    #[test]
    fn defaults_user_scope_no_tools() {
        let a = args_of(&["voidcrawl-mcp", "install"]);
        assert_eq!(a.scope, Scope::User);
        assert!(a.tool.is_empty());
        assert!(!a.dry_run);
        assert!(!a.status);
    }

    #[test]
    fn parses_scope_repeated_tool_and_flags() {
        let a = args_of(&[
            "voidcrawl-mcp",
            "install",
            "--scope",
            "project",
            "--tool",
            "codex",
            "--tool",
            "opencode",
            "--dry-run",
            "--status",
        ]);
        assert_eq!(a.scope, Scope::Project);
        assert_eq!(a.tool, vec![Host::Codex, Host::Opencode]);
        assert!(a.dry_run);
        assert!(a.status);
    }

    #[test]
    fn empty_tool_resolves_to_all_hosts() {
        // The run() builder fans an empty --tool out to every host.
        let a = args_of(&["voidcrawl-mcp", "install"]);
        let hosts = if a.tool.is_empty() { Host::ALL.to_vec() } else { a.tool.clone() };
        assert_eq!(hosts, Host::ALL.to_vec());
    }

    #[test]
    fn rejects_bad_enum_value() {
        assert!(
            TestCli::try_parse_from(["voidcrawl-mcp", "install", "--scope", "global"]).is_err()
        );
        assert!(TestCli::try_parse_from(["voidcrawl-mcp", "install", "--tool", "vim"]).is_err());
    }

    #[test]
    fn opencode_merge_preserves_other_servers() {
        let existing = r#"{ "lsp": true, "mcp": { "other": { "type": "local" } } }"#;
        let merged = opencode_merge(Some(existing), "/abs/voidcrawl-mcp").unwrap();
        let mcp = merged.get("mcp").unwrap().as_object().unwrap();
        // existing server survived
        assert!(mcp.contains_key("other"));
        // top-level key survived
        assert_eq!(merged.get("lsp"), Some(&Value::Bool(true)));
        // ours is wired with the absolute exe path and the right shape
        let ours = mcp.get("voidcrawl").unwrap();
        assert_eq!(ours.get("type").unwrap(), "local");
        assert_eq!(ours.get("command").unwrap(), &Value::Array(vec!["/abs/voidcrawl-mcp".into()]));
        assert_eq!(ours.get("enabled").unwrap(), &Value::Bool(true));
        assert_eq!(ours.get("environment").unwrap().get("CHROME_HEADLESS").unwrap(), "1");
    }

    #[test]
    fn opencode_merge_from_empty_makes_valid_root() {
        let merged = opencode_merge(None, "/abs/voidcrawl-mcp").unwrap();
        assert!(merged.get("mcp").unwrap().get("voidcrawl").is_some());
    }

    #[test]
    fn opencode_remove_drops_only_ours() {
        let existing = r#"{ "mcp": { "voidcrawl": {}, "other": {} } }"#;
        let pruned = opencode_remove(existing).unwrap();
        let mcp = pruned.get("mcp").unwrap().as_object().unwrap();
        assert!(!mcp.contains_key("voidcrawl"));
        assert!(mcp.contains_key("other"));
    }

    #[test]
    fn manual_blocks_embed_absolute_exe_path() {
        let exe = "/home/u/.cargo/bin/voidcrawl-mcp";
        assert!(claude_manual_json(exe).unwrap().contains(exe));
        assert!(opencode_manual_json(exe).unwrap().contains(exe));
        let toml = codex_manual_toml(exe);
        assert!(toml.contains(exe));
        assert!(toml.contains("[mcp_servers.voidcrawl]"));
        assert!(toml.contains("CHROME_HEADLESS = \"1\""));
    }

    #[test]
    fn claude_add_argv_terminates_command_after_double_dash() {
        let argv = claude_add_argv(Scope::User, "/abs/voidcrawl-mcp");
        let dash = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(argv.last().unwrap(), "/abs/voidcrawl-mcp");
        assert!(argv[..dash].contains(&"--scope".to_string()));
        assert!(argv[..dash].contains(&"user".to_string()));
    }
}
