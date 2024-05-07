use std::path::{Path, PathBuf};

use object::{Object, ObjectSymbol};
use tera::{Context, Tera};

#[derive(clap::Parser)]
pub struct CliConfig {
    pub binary: PathBuf,
    #[arg(long)]
    pub runner_cfg: Option<PathBuf>,
    #[arg(long, short = 'v')]
    pub verbose: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub load: String,
    #[serde(alias = "openocd-cfg")]
    pub openocd_cfg: Option<PathBuf>,
    #[serde(alias = "openocd-log")]
    pub openocd_log: Option<PathBuf>,
    #[serde(alias = "pre-runner")]
    pub pre_runner: Option<Command>,
    #[serde(alias = "post-runner")]
    pub post_runner: Option<Command>,
    #[serde(alias = "rtt-port")]
    pub rtt_port: Option<u16>,
    pub mantra: Option<MantraConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MantraConfig {
    #[serde(alias = "db-url")]
    pub db_url: Option<String>,
    pub extract: Option<mantra::cmd::extract::Config>,
    pub extern_traces: Option<Vec<PathBuf>>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, serde::Deserialize)]
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

impl Config {
    pub fn gdb_script(&self, binary: &Path) -> Result<String, CfgError> {
        let resolved_load = resolve_load(&self.load, binary)?;
        let (rtt_address, rtt_length) = find_rtt_block(binary)?;

        #[cfg(target_os = "windows")]
        let sleep_cmd = "timeout";
        #[cfg(not(target_os = "windows"))]
        let sleep_cmd = "sleep";

        Ok(format!(
            "
set pagination off

target extended-remote | openocd -c \"gdb_port pipe; log_output {}\" -f {}

{}

b main
continue

monitor rtt setup 0x{:x} {} \"SEGGER RTT\"
monitor rtt start
monitor rtt server start {} 0

shell {sleep_cmd} 1

continue

shell {sleep_cmd} 1

quit        
",
            self.openocd_log
                .clone()
                .unwrap_or(PathBuf::from(".embedded/openocd.log"))
                .to_string_lossy(),
            self.openocd_cfg
                .clone()
                .unwrap_or(PathBuf::from(".embedded/openocd.cfg"))
                .to_string_lossy(),
            resolved_load,
            rtt_address,
            rtt_length,
            self.rtt_port.unwrap_or(super::DEFAULT_RTT_PORT)
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
    context.insert("binary_path", &parent);
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
        ),
    );
    context.insert("binary_filepath", &binary);

    Tera::one_off(load, &context, false).map_err(|err| {
        CfgError::ResolvingLoadSection(format!("Failed rendering the load template. Cause: {err}"))
    })
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use super::{find_rtt_block, resolve_load};

    #[cfg(target_os = "windows")]
    #[test]
    fn load_template() {
        let load = "load \"{{ binary_path }}\\debug_config.ihex\"
load \"{{ binary_filepath_noextension }}.ihex\"
file \"{{ binary_filepath }}\"";

        let binary = PathBuf::from(".\\target\\debug\\hello.exe");

        let resolved = resolve_load(load, &binary).unwrap();

        assert!(
            resolved.contains("target\\debug\\debug_config.ihex"),
            "Binary path not resolved."
        );
        assert!(
            resolved.contains("target\\debug\\hello.ihex"),
            "Binary file path without extension not resolved."
        );
        assert!(
            resolved.contains("target\\debug\\hello.exe"),
            "Binary file path with extension not resolved."
        );
    }

    #[cfg(not(target_os = "windows"))]
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
