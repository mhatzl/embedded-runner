use std::path::{Path, PathBuf};

use object::{Object, ObjectSymbol};
use path_slash::{PathBufExt, PathExt};
use tera::{Context, Tera};

#[derive(Debug, Clone, clap::Parser)]
pub struct CliConfig {
    #[arg(long, short = 'v')]
    pub verbose: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub runner_cfg: RunnerConfig,
    pub verbose: bool,
    pub workspace_dir: PathBuf,
    pub embedded_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{}", .0)]
    Path(#[from] crate::path::PathError),
    #[error("{}", .0)]
    Fs(#[from] std::io::Error),
    #[error("{}", .0)]
    DeToml(#[from] toml::de::Error),
}

pub fn get_cfg(runner_cfg: &Option<PathBuf>, verbose: bool) -> Result<ResolvedConfig, ConfigError> {
    let workspace_dir = crate::path::get_cargo_root()?;
    let embedded_dir: PathBuf = workspace_dir.join(".embedded/");

    if !embedded_dir.exists() {
        std::fs::create_dir(embedded_dir.clone())?;
    }

    let runner_cfg = runner_cfg
        .clone()
        .unwrap_or(embedded_dir.join("runner.toml"));
    let runner_cfg: RunnerConfig = match std::fs::read_to_string(&runner_cfg) {
        Ok(runner_cfg) => toml::from_str(&runner_cfg)?,
        Err(_) => {
            log::warn!(
                "No runner config found at '{}'. Using default config.",
                runner_cfg.display()
            );
            RunnerConfig::default()
        }
    };

    Ok(ResolvedConfig {
        runner_cfg,
        verbose,
        workspace_dir,
        embedded_dir,
    })
}

#[derive(Debug, Clone, clap::Parser)]
pub enum Cmd {
    Run(RunCmdConfig),
    Collect(CollectCmdConfig),
}

#[derive(Debug, Clone, clap::Parser)]
pub struct RunCmdConfig {
    #[arg(long)]
    pub runner_cfg: Option<PathBuf>,
    /// `true`: Uses RTT commands to communicate with SEGGER GDB instead of the `monitor rtt` commands from OpenOCD.
    #[arg(long)]
    pub segger_gdb: bool,
    #[arg(long)]
    pub run_name: Option<String>,
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
    /// Path to look for JSON metadata that is linked with the test run.
    /// The default is to look for the file at `.embedded/meta.json`.
    #[arg(long)]
    pub meta_filepath: Option<PathBuf>,
    pub binary: PathBuf,
}

#[derive(Debug, Clone, clap::Parser)]
pub struct CollectCmdConfig {
    pub output: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct RunnerConfig {
    pub load: Option<String>,
    #[serde(alias = "openocd-cfg")]
    pub openocd_cfg: Option<PathBuf>,
    #[serde(alias = "gdb-connection")]
    pub gdb_connection: Option<String>,
    #[serde(alias = "gdb-logfile")]
    pub gdb_logfile: Option<PathBuf>,
    #[serde(alias = "pre-runner")]
    pub pre_runner: Option<Command>,
    #[serde(alias = "pre-runner-windows")]
    pub pre_runner_windows: Option<Command>,
    #[serde(alias = "post-runner")]
    pub post_runner: Option<Command>,
    #[serde(alias = "post-runner-windows")]
    pub post_runner_windows: Option<Command>,
    #[serde(alias = "rtt-port")]
    pub rtt_port: Option<u16>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CfgError {
    #[error("Could not find rtt block in binary. Cause: {}", .0)]
    FindingRttBlock(String),
    #[error("Could not resolve the load section. Cause: {}", .0)]
    ResolvingLoadSection(String),
}

impl RunnerConfig {
    pub fn gdb_script(
        &self,
        binary: &Path,
        output_dir: &Path,
        segger_gdb: bool,
    ) -> Result<String, CfgError> {
        let resolved_load = if let Some(load) = &self.load {
            resolve_load(load, binary)?
        } else {
            "load".to_string()
        };
        let (rtt_address, rtt_length) = find_rtt_block(binary)?;

        #[cfg(target_os = "windows")]
        let sleep_cmd = "timeout";
        #[cfg(not(target_os = "windows"))]
        let sleep_cmd = "sleep";

        let gdb_logfile = self
            .gdb_logfile
            .clone()
            .unwrap_or(output_dir.join("gdb.log"));
        let gdb_logfile = gdb_logfile
            .to_slash()
            .expect("GDB logfile must be a valid filepath.");

        let gdb_conn = if let Some(gdb_conn) = &self.gdb_connection {
            format!("target extended-remote {gdb_conn}\nset logging file {gdb_logfile}")
        } else {
            let openocd_cfg = self
                .openocd_cfg
                .clone()
                .unwrap_or(PathBuf::from(".embedded/openocd.cfg"));
            let openocd_cfg = openocd_cfg
                .to_slash()
                .expect("OpenOCD configuration file must be a valid filepath.");
            format!("target extended-remote | openocd -c \"gdb_port pipe; log_output {gdb_logfile}\" -f {openocd_cfg}")
        };

        let rtt_section = if segger_gdb {
            format!(
                "
monitor exec SetRTTSearchRanges 0x{:x} 0x{:x}
monitor exec SetRTTChannel 0
            ",
                rtt_address, rtt_length
            )
        } else {
            format!(
                "
monitor rtt setup 0x{:x} {} \"SEGGER RTT\"
monitor rtt start
monitor rtt server start {} 0
            ",
                rtt_address,
                rtt_length,
                self.rtt_port.unwrap_or(super::DEFAULT_RTT_PORT)
            )
        };

        Ok(format!(
            "
set pagination off

{}

{}

b main
continue

{rtt_section}

shell {sleep_cmd} 1

continue

shell {sleep_cmd} 1

quit        
",
            gdb_conn, resolved_load
        ))
    }
}

fn find_rtt_block(binary: &Path) -> Result<(u64, u64), CfgError> {
    let data = std::fs::read(binary).map_err(|err| {
        CfgError::FindingRttBlock(format!("Could not read binary file. Cause: {err}"))
    })?;
    let file = object::File::parse(&*data).map_err(|err| {
        CfgError::FindingRttBlock(format!("Could not parse binary file. Cause: {err}"))
    })?;

    for symbol in file.symbols() {
        if symbol.name() == Ok("_SEGGER_RTT") {
            return Ok((symbol.address(), symbol.size()));
        }
    }

    Err(CfgError::FindingRttBlock(
        "No _SEGGER_RTT symbol in binary!".to_string(),
    ))
}

fn resolve_load(load: &str, binary: &Path) -> Result<String, CfgError> {
    let mut context = Context::new();
    let parent = binary.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    context.insert(
        "binary_path",
        &parent
            .to_slash()
            .expect("Binary path has only valid Unicode characters."),
    );
    context.insert(
        "binary_filepath_noextension",
        &Path::join(
            &parent,
            binary.file_stem().ok_or_else(|| {
                CfgError::ResolvingLoadSection(format!(
                    "Given binary '{}' has no valid filename.",
                    binary.display()
                ))
            })?,
        )
        .to_slash()
        .expect("Binary path has only valid Unicode characters."),
    );
    context.insert(
        "binary_filepath",
        &binary
            .to_slash()
            .expect("Binary path has only valid Unicode characters."),
    );

    Tera::one_off(load, &context, false).map_err(|err| {
        CfgError::ResolvingLoadSection(format!("Failed rendering the load template. Cause: {err}"))
    })
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use super::{find_rtt_block, resolve_load};

    #[test]
    fn load_template() {
        let load = "load \"{{ binary_path }}/debug_config.ihex\"
load \"{{ binary_filepath_noextension }}.ihex\"
file \"{{ binary_filepath }}\"";

        let binary = PathBuf::from("./target/debug/hello.exe");

        let resolved = resolve_load(load, &binary).unwrap();

        assert!(
            resolved.contains("target/debug/debug_config.ihex"),
            "Binary path not resolved."
        );
        assert!(
            resolved.contains("target/debug/hello.ihex"),
            "Binary file path without extension not resolved."
        );
        assert!(
            resolved.contains("target/debug/hello.exe"),
            "Binary file path with extension not resolved."
        );
    }

    #[test]
    fn rtt_block_in_binary() {
        let binary = PathBuf::from("test_binaries/emb-runner-test");

        let (address, size) = find_rtt_block(&binary).unwrap();
        dbg!(address);
        dbg!(size);
    }
}
