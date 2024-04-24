use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::AtomicBool, Arc},
};

use cfg::{CliConfig, Config};
use defmt_json_schema::v1::JsonFrame;
use mantra::db::GitRepoOrigin;

pub mod cfg;
pub mod defmt;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Timeout waiting for rtt connection to start.")]
    RttTimeout,
    #[error("Error from gdb: {}", .0)]
    Gdb(String),
    #[error("Error setting up the gdp script: {}", .0)]
    GdbScript(String),
    #[error("Error reading the runner config: {}", .0.display())]
    ReadingCfg(PathBuf),
    #[error("Error parsing the runner config: {}", .0)]
    ParsingCfg(String),
    #[error("Error reading defmt logs: {}", .0)]
    Defmt(String),
    #[error("Error adding coverage data to mantra: {}", .0)]
    Mantra(String),
    #[error("Failed setting up embedded-runner. Cause: {}", .0)]
    Setup(String),
    #[error("Failed executing pre runner. Cause: {}", .0)]
    PreRunner(String),
    #[error("Failed executing post runner. Cause: {}", .0)]
    PostRunner(String),
}

pub const DEFAULT_RTT_PORT: u16 = 19021;

pub async fn run(cli_cfg: CliConfig) -> Result<(), RunnerError> {
    let binary_str = cli_cfg.binary.display().to_string();
    let embedded_dir: PathBuf = PathBuf::from(".embedded/");

    if !embedded_dir.exists() {
        std::fs::create_dir(embedded_dir.clone()).map_err(|err| {
            RunnerError::Setup(format!(
                "Could not create directory '.embedded/'. Cause: {err}"
            ))
        })?;
    }

    let runner_cfg = cli_cfg
        .runner_cfg
        .unwrap_or(embedded_dir.join("runner.toml"));
    let runner_cfg: cfg::Config = match std::fs::read_to_string(&runner_cfg) {
        Ok(runner_cfg) => {
            toml::from_str(&runner_cfg).map_err(|err| RunnerError::ParsingCfg(err.to_string()))?
        }
        Err(_) => return Err(RunnerError::ReadingCfg(runner_cfg)),
    };

    if let Some(pre_command) = &runner_cfg.pre_runner {
        println!("--------------- Pre Runner --------------------");
        let mut args = pre_command.args.clone();
        args.push(binary_str.clone());

        let output = std::process::Command::new(&pre_command.name)
            .args(args)
            .output()
            .map_err(|err| RunnerError::PreRunner(err.to_string()))?;
        print!(
            "{}",
            String::from_utf8(output.stdout).expect("Stdout must be valid utf8.")
        );
        eprint!(
            "{}",
            String::from_utf8(output.stderr).expect("Stderr must be valid utf8.")
        );

        if !output.status.success() {
            return Err(RunnerError::PreRunner(format!(
                "Returned with exit code: {}",
                output.status,
            )));
        }
    }

    let gdb_script = runner_cfg
        .gdb_script(&cli_cfg.binary)
        .map_err(|_err| RunnerError::GdbScript(String::new()))?;

    let gdb_script_file = embedded_dir.join("embedded.gdb");
    std::fs::write(&gdb_script_file, gdb_script)
        .map_err(|err| RunnerError::GdbScript(err.to_string()))?;

    let (defmt_frames, gdb_result) =
        run_gdb_sequence(cli_cfg.binary, &gdb_script_file, &runner_cfg)?;
    let gdb_status = gdb_result?;

    if !gdb_status.success() {
        return Err(RunnerError::Gdb(format!(
            "GDB did not run successfully. Exit code: '{gdb_status}'"
        )));
    }

    println!("--------------- Logs --------------------");
    for frame in &defmt_frames {
        let location = if frame.location.file.is_some()
            && frame.location.line.is_some()
            && frame.location.module_path.is_some()
        {
            let mod_path = frame.location.module_path.as_ref().unwrap();

            format!(
                "{}:{} in {}::{}::{}",
                frame.location.file.as_ref().unwrap(),
                frame.location.line.unwrap(),
                mod_path.crate_name,
                mod_path.modules.join("::"),
                mod_path.function,
            )
        } else {
            "no-location".to_string()
        };
        match frame.level {
            Some(level) => log::log!(level, "{}\n@{}", frame.data, location),
            None => println!("{}\n@{}", frame.data, location),
        }
    }

    // option: add mantra coverage, if run as test (first defmt log starts with "(nr/nr) running `test fn`...")
    if let Some(mantra_cfg) = runner_cfg.mantra {
        let db_url = mantra_cfg
            .db_url
            .unwrap_or("sqlite://.embedded/mantra.db?mode=rwc".to_string());
        let db = mantra::db::MantraDb::new(&mantra::db::Config { url: Some(db_url) })
            .await
            .map_err(|err| RunnerError::Mantra(err.to_string()))?;

        if let Some(extract_cfg) = mantra_cfg.extract {
            mantra::cmd::extract::extract(&db, &extract_cfg)
                .await
                .map_err(|err| RunnerError::Mantra(err.to_string()))?;
        }

        db.add_project(
            &mantra_cfg.project_name,
            mantra::db::ProjectOrigin::GitRepo(GitRepoOrigin {
                link: std::env::var("CARGO_PKG_REPOSITORY").unwrap_or("local".to_string()),
                branch: None,
            }),
        )
        .await
        .map_err(|err| RunnerError::Mantra(err.to_string()))?;

        let current_dir = std::env::current_dir().unwrap_or_default();
        mantra::cmd::trace::trace(
            &db,
            &mantra::cmd::trace::Config {
                root: current_dir.clone(),
                project_name: mantra_cfg.project_name.clone(),
            },
        )
        .await
        .map_err(|err| RunnerError::Mantra(err.to_string()))?;

        mantra::cmd::coverage::coverage_from_defmt_frames(
            &defmt_frames,
            &db,
            &mantra_cfg.project_name,
            mantra_cfg.test_prefix.as_deref(),
        )
        .await
        .map_err(|err| RunnerError::Mantra(err.to_string()))?;
    }

    if let Some(post_command) = &runner_cfg.post_runner {
        println!("--------------- Post Runner --------------------");
        let mut args = post_command.args.clone();
        args.push(binary_str);

        let output = std::process::Command::new(&post_command.name)
            .args(args)
            .output()
            .map_err(|err| RunnerError::PostRunner(err.to_string()))?;
        print!(
            "{}",
            String::from_utf8(output.stdout).expect("Stdout must be valid utf8.")
        );
        eprint!(
            "{}",
            String::from_utf8(output.stderr).expect("Stderr must be valid utf8.")
        );

        if !output.status.success() {
            return Err(RunnerError::PostRunner(format!(
                "Returned with exit code: {}",
                output.status,
            )));
        }
    }

    Ok(())
}

#[inline]
pub fn run_gdb_sequence(
    binary: PathBuf,
    tmp_gdb_file: &Path,
    runner_cfg: &Config,
) -> Result<
    (
        Vec<JsonFrame>,
        Result<std::process::ExitStatus, RunnerError>,
    ),
    RunnerError,
> {
    let mut gdb_cmd = Command::new("arm-none-eabi-gdb");
    let mut gdb = gdb_cmd
        .args([
            "-x",
            &tmp_gdb_file.to_string_lossy(),
            &binary.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut open_ocd_output = gdb.stderr.take().unwrap();

    let mut buf = [0; 100];
    let mut content = Vec::new();
    let rtt_start = b"for rtt connection";

    println!("--------------- OpenOCD --------------------");
    let start = std::time::Instant::now();
    'outer: while let Ok(n) = open_ocd_output.read(&mut buf) {
        if n > 0 {
            content.extend_from_slice(&buf[..n]);

            print!("{}", String::from_utf8_lossy(&buf[..n]));

            for i in 0..n {
                let slice_end = content.len().saturating_sub(i);
                if content[..slice_end].ends_with(rtt_start) {
                    break 'outer;
                }
            }
        } else if std::time::Instant::now()
            .checked_duration_since(start)
            .unwrap()
            .as_millis()
            > 12000
        {
            log::error!("Timeout while waiting for rtt connection.");
            let _ = gdb.kill();
            return Err(RunnerError::RttTimeout);
        }
    }
    println!();

    // start defmt thread + end-signal
    let end_signal = Arc::new(AtomicBool::new(false));
    let thread_signal = end_signal.clone();
    let rtt_port = runner_cfg.rtt_port.unwrap_or(DEFAULT_RTT_PORT);
    let defmt_thread =
        std::thread::spawn(move || defmt::read_defmt_frames(&binary, rtt_port, thread_signal));

    // wait for gdb to end
    let gdb_result = gdb
        .wait()
        .map_err(|err| RunnerError::Gdb(format!("Error waiting for gdb to finish. Cause: {err}")));

    // signal defmt end
    end_signal.store(true, std::sync::atomic::Ordering::Relaxed);

    // join defmt thread to get logs
    let defmt_result = defmt_thread
        .join()
        .map_err(|_| RunnerError::Defmt("Failed waiting for defmt logs.".to_string()))?;

    // print logs
    let defmt_frames = defmt_result
        .map_err(|err| RunnerError::Defmt(format!("Failed extracting defmt logs. Cause: {err}")))?;

    Ok((defmt_frames, gdb_result))
}
