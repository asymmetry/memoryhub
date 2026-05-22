//! Command-line interface for the MemoryHub binary.

use std::path::PathBuf;

use clap::Parser;

use memoryhub::config::Config;

/// Centralized memory service for teams of AI agents.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Root directory for all data [default: $MEMORYHUB_HOME or ~/.memoryhub]
    #[arg(long, value_name = "PATH")]
    pub base_dir: Option<PathBuf>,

    /// Config file to load [default: {base-dir}/config.toml]
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Override the server bind host
    #[arg(long, value_name = "HOST")]
    pub host: Option<String>,

    /// Override the server bind port
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,

    /// Log filter, e.g. `info` or `memoryhub=debug` (overrides RUST_LOG)
    #[arg(long, value_name = "FILTER")]
    pub log_level: Option<String>,
}

impl Cli {
    /// Applies the command-line overrides onto a loaded configuration.
    ///
    /// Each flag takes precedence over the corresponding config-file value when present.
    pub fn apply_overrides(&self, config: &mut Config) {
        if let Some(host) = &self.host {
            config.server.host = host.clone();
        }
        if let Some(port) = self.port {
            config.server.port = port;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_are_none() {
        let cli = Cli::try_parse_from(["memoryhub"]).unwrap();
        assert!(cli.base_dir.is_none());
        assert!(cli.config.is_none());
        assert!(cli.host.is_none());
        assert!(cli.port.is_none());
        assert!(cli.log_level.is_none());
    }

    #[test]
    fn test_flags_parse() {
        let cli = Cli::try_parse_from([
            "memoryhub",
            "--base-dir",
            "/data/mh",
            "--config",
            "/tmp/c.toml",
            "--host",
            "127.0.0.1",
            "--port",
            "9000",
            "--log-level",
            "memoryhub=debug",
        ])
        .unwrap();
        assert_eq!(cli.base_dir, Some(PathBuf::from("/data/mh")));
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/c.toml")));
        assert_eq!(cli.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(cli.port, Some(9000));
        assert_eq!(cli.log_level.as_deref(), Some("memoryhub=debug"));
    }

    #[test]
    fn test_invalid_port_rejected() {
        let result = Cli::try_parse_from(["memoryhub", "--port", "not-a-number"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_overrides_sets_present_fields() {
        let cli =
            Cli::try_parse_from(["memoryhub", "--host", "1.2.3.4", "--port", "9000"]).unwrap();
        let mut config = Config::default();
        cli.apply_overrides(&mut config);
        assert_eq!(config.server.host, "1.2.3.4");
        assert_eq!(config.server.port, 9000);
    }

    #[test]
    fn test_apply_overrides_leaves_absent_fields() {
        let cli = Cli::try_parse_from(["memoryhub"]).unwrap();
        let mut config = Config::default();
        let default = Config::default();
        cli.apply_overrides(&mut config);
        assert_eq!(config.server.host, default.server.host);
        assert_eq!(config.server.port, default.server.port);
    }
}
