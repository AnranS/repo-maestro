//! `maestro list` / `maestro stop` — discover and stop the maestro services
//! running on this host (web UI daemons, in-flight runs).
//!
//! Discovery is process-based (parse `ps`), the same approach `maestro where`
//! already uses, so there's no registry file to keep in sync or clean up after
//! a crash. We only surface *long-running* subcommands (ui/tui/work/run); the
//! transient CLI invocations (`list`, `stop`, `where`, …) are filtered out.

use anyhow::{bail, Result};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Ui,
    Tui,
    Work,
    Run,
}

impl ServiceKind {
    fn from_subcommand(tok: &str) -> Option<Self> {
        match tok {
            "ui" => Some(Self::Ui),
            "tui" => Some(Self::Tui),
            "work" => Some(Self::Work),
            "run" => Some(Self::Run),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Tui => "tui",
            Self::Work => "work",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServiceProc {
    pub pid: u32,
    pub kind: ServiceKind,
    pub port: Option<u16>,
    pub cwd: Option<String>,
}

/// Enumerate maestro service processes on this host (excluding ourselves).
pub fn discover() -> Vec<ServiceProc> {
    // `ps -ax -o pid=,command=` → "  1234 /path/maestro ui --port 7777"
    let out = Command::new("ps")
        .args(["-ax", "-o", "pid=,command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let self_pid = std::process::id();
    let mut found = Vec::new();
    for line in out.lines() {
        let line = line.trim_start();
        let Some((pid_str, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid_str.trim().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let rest = rest.trim();
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        // Find the maestro binary token, then read the subcommand after it.
        let Some(bin_idx) = tokens.iter().position(|t| is_maestro_bin(t)) else {
            continue;
        };
        let Some(sub) = tokens.get(bin_idx + 1) else {
            continue;
        };
        let Some(kind) = ServiceKind::from_subcommand(sub) else {
            continue;
        };
        found.push(ServiceProc {
            pid,
            kind,
            port: parse_port(&tokens[bin_idx..]),
            cwd: process_cwd(pid),
        });
    }
    found.sort_by_key(|s| s.pid);
    found
}

fn is_maestro_bin(tok: &str) -> bool {
    let name = tok.rsplit('/').next().unwrap_or(tok);
    name == "maestro" || name == "mst"
}

/// Pull the port out of `--port N` / `--port=N` (the ui/tui servers).
fn parse_port(tokens: &[&str]) -> Option<u16> {
    let mut it = tokens.iter();
    while let Some(t) = it.next() {
        if let Some(v) = t.strip_prefix("--port=") {
            return v.parse().ok();
        }
        if *t == "--port" {
            return it.next().and_then(|v| v.parse().ok());
        }
    }
    None
}

/// Best-effort working directory for a pid: `/proc` on Linux, `lsof` on macOS.
fn process_cwd(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
            return Some(p.to_string_lossy().to_string());
        }
    }
    let out = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n').map(str::to_string))
}

/// `maestro list`
pub fn cmd_list() -> Result<()> {
    let services = discover();
    if services.is_empty() {
        println!("No maestro services running on this host.");
        return Ok(());
    }
    println!("{:>7}  {:<5}  {:<22}  WORKSPACE", "PID", "KIND", "ADDRESS");
    for s in &services {
        let address = match s.port {
            Some(p) => format!("http://127.0.0.1:{p}"),
            None => "-".to_string(),
        };
        let cwd = s.cwd.as_deref().unwrap_or("-");
        println!(
            "{:>7}  {:<5}  {:<22}  {}",
            s.pid,
            s.kind.label(),
            address,
            cwd
        );
    }
    println!("\nStop one with `maestro stop <pid>` or `maestro stop --port <port>`; all with `maestro stop --all`.");
    Ok(())
}

#[derive(clap::Parser, Debug)]
pub struct StopArgs {
    /// PID of the service to stop (see `maestro list`).
    pub pid: Option<u32>,

    /// Stop the service listening on this port.
    #[arg(long)]
    pub port: Option<u16>,

    /// Stop every maestro service on this host.
    #[arg(long)]
    pub all: bool,

    /// Send SIGKILL instead of a graceful SIGTERM.
    #[arg(long)]
    pub force: bool,
}

/// `maestro stop`
pub fn cmd_stop(args: StopArgs) -> Result<()> {
    let services = discover();
    if services.is_empty() {
        println!("No maestro services running — nothing to stop.");
        return Ok(());
    }

    let targets: Vec<&ServiceProc> = if args.all {
        services.iter().collect()
    } else if let Some(port) = args.port {
        let matched: Vec<&ServiceProc> = services.iter().filter(|s| s.port == Some(port)).collect();
        if matched.is_empty() {
            bail!("no maestro service is listening on port {port} (see `maestro list`)");
        }
        matched
    } else if let Some(pid) = args.pid {
        match services.iter().find(|s| s.pid == pid) {
            Some(s) => vec![s],
            None => bail!("pid {pid} is not a maestro service (see `maestro list`)"),
        }
    } else if services.len() == 1 {
        // Unambiguous: stop the only service running.
        services.iter().collect()
    } else {
        println!("Several maestro services are running — specify which to stop:\n");
        cmd_list()?;
        bail!("refusing to guess; pass a <pid>, --port, or --all");
    };

    let signal = if args.force { "-9" } else { "-15" };
    let mut stopped = 0;
    for s in &targets {
        let ok = Command::new("kill")
            .arg(signal)
            .arg(s.pid.to_string())
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if ok {
            let addr = s.port.map(|p| format!(" (:{p})")).unwrap_or_default();
            println!("✓ stopped {} pid {}{}", s.kind.label(), s.pid, addr);
            stopped += 1;
        } else {
            eprintln!("✗ failed to stop pid {} (already gone?)", s.pid);
        }
    }
    if stopped == 0 {
        bail!("nothing was stopped");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_both_forms() {
        assert_eq!(parse_port(&["ui", "--port", "7788"]), Some(7788));
        assert_eq!(parse_port(&["ui", "--port=8080"]), Some(8080));
        assert_eq!(parse_port(&["work", "--run"]), None);
    }

    #[test]
    fn recognises_maestro_binaries() {
        assert!(is_maestro_bin("maestro"));
        assert!(is_maestro_bin("/usr/local/bin/maestro"));
        assert!(is_maestro_bin("mst"));
        assert!(is_maestro_bin("./target/release/mst"));
        assert!(!is_maestro_bin("maestromatic"));
        assert!(!is_maestro_bin("git"));
    }

    #[test]
    fn maps_known_subcommands_only() {
        assert_eq!(ServiceKind::from_subcommand("ui"), Some(ServiceKind::Ui));
        assert_eq!(
            ServiceKind::from_subcommand("work"),
            Some(ServiceKind::Work)
        );
        assert_eq!(ServiceKind::from_subcommand("list"), None);
        assert_eq!(ServiceKind::from_subcommand("stop"), None);
    }
}
