use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::AtomicBool, Arc},
};

use cfg::{CliConfig, Config, RunConfig, RunnerConfig};
use defmt_json_schema::v1::JsonFrame;
use path_clean::PathClean;

pub mod cfg;
pub mod defmt;

pub async fn run(cli_cfg: CliConfig) -> Result<(), RunnerError> {
    let cfg = get_cfg(&cli_cfg)?;

    match cli_cfg.cmd {
        cfg::Cmd::Run(run_cfg) => run_cmd(&cfg, run_cfg).await,
        cfg::Cmd::Mantra(mantra_cmd) => {
            let mantra_cfg = mantra::cfg::Config {
                db: mantra::db::Config {
                    url: Some(mantra_db_url(
                        cfg.runner_cfg.mantra.and_then(|m| m.db_url),
                        &cfg.embedded_dir,
                    )),
                },
                cmd: mantra_cmd.clone(),
            };

            mantra::run(mantra_cfg)
                .await
                .map_err(|err| RunnerError::Mantra(err.to_string()))
        }
    }
}

pub fn get_cfg(cli_cfg: &CliConfig) -> Result<Config, RunnerError> {
    let workspace_dir = mantra::path::get_cargo_root()
        .map_err(|_| RunnerError::Mantra("No workspace directory found.".to_string()))?;
    let embedded_dir: PathBuf = workspace_dir.join(".embedded/");

    if !embedded_dir.exists() {
        std::fs::create_dir(embedded_dir.clone()).map_err(|err| {
            RunnerError::Setup(format!(
                "Could not create directory '{}'. Cause: {}",
                embedded_dir.display(),
                err
            ))
        })?;
    }

    let runner_cfg = cli_cfg
        .runner_cfg
        .clone()
        .unwrap_or(embedded_dir.join("runner.toml"));
    let runner_cfg: cfg::RunnerConfig = match std::fs::read_to_string(&runner_cfg) {
        Ok(runner_cfg) => {
            toml::from_str(&runner_cfg).map_err(|err| RunnerError::ParsingCfg(err.to_string()))?
        }
        Err(_) => return Err(RunnerError::ReadingCfg(runner_cfg)),
    };

    Ok(Config {
        runner_cfg,
        verbose: cli_cfg.verbose,
        workspace_dir,
        embedded_dir,
    })
}

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

pub async fn run_cmd(main_cfg: &Config, run_cfg: RunConfig) -> Result<(), RunnerError> {
    let binary_str = run_cfg.binary.display().to_string();

    if let Some(pre_command) = &main_cfg.runner_cfg.pre_runner {
        println!("--------------- Pre Runner --------------------");
        let mut args = pre_command.args.clone();
        args.push(binary_str.clone());

        let output = std::process::Command::new(&pre_command.name)
            .args(args)
            .current_dir(&main_cfg.workspace_dir)
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

    let gdb_script = main_cfg
        .runner_cfg
        .gdb_script(&run_cfg.binary)
        .map_err(|_err| RunnerError::GdbScript(String::new()))?;

    let gdb_script_file = main_cfg.embedded_dir.join("embedded.gdb");
    std::fs::write(&gdb_script_file, gdb_script)
        .map_err(|err| RunnerError::GdbScript(err.to_string()))?;

    let (defmt_frames, gdb_result) = run_gdb_sequence(
        run_cfg.binary,
        &main_cfg.workspace_dir,
        &gdb_script_file,
        &main_cfg.runner_cfg,
    )?;
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

    if let Some(mantra_cfg) = &main_cfg.runner_cfg.mantra {
        println!("------------- Mantra -------------");

        let db_url = mantra_db_url(mantra_cfg.db_url.clone(), &main_cfg.embedded_dir);
        let db = mantra::db::MantraDb::new(&mantra::db::Config { url: Some(db_url) })
            .await
            .map_err(|err| RunnerError::Mantra(err.to_string()))?;

        if let Some(extract_cfg) = &mantra_cfg.extract {
            let req_changes = mantra::cmd::extract::extract(&db, extract_cfg)
                .await
                .map_err(|err| RunnerError::Mantra(err.to_string()))?;

            let deleted_reqs = db
                .delete_req_generations(req_changes.new_generation)
                .await
                .map_err(|err| RunnerError::Mantra(err.to_string()))?;
            db.reset_req_generation().await;

            if main_cfg.verbose {
                println!("{req_changes}");

                if let Some(deleted) = deleted_reqs {
                    println!("{deleted}");
                }
            }
        }

        let mut changes = mantra::cmd::trace::trace(
            &db,
            &mantra::cmd::trace::Config {
                root: main_cfg.workspace_dir.clone(),
                keep_root_absolute: false,
            },
        )
        .await
        .map_err(|err| RunnerError::Mantra(err.to_string()))?;

        let first_generation = changes.new_generation;

        if let Some(extern_traces) = &mantra_cfg.extern_traces {
            for trace_root in extern_traces {
                match absolute_path(trace_root) {
                    Ok(abs_path) => {
                        let mut extern_changes = mantra::cmd::trace::trace(
                            &db,
                            &mantra::cmd::trace::Config {
                                root: abs_path,
                                keep_root_absolute: true,
                            },
                        )
                        .await
                        .map_err(|err| RunnerError::Mantra(err.to_string()))?;

                        changes.merge(&mut extern_changes);
                    }
                    Err(_) => {
                        log::error!("Skipped bad extern trace root '{}'.", trace_root.display());
                    }
                }
            }
        }

        let deleted_traces = db
            .delete_trace_generations(first_generation)
            .await
            .map_err(|err| RunnerError::Mantra(err.to_string()))?;
        db.reset_trace_generation().await;

        if main_cfg.verbose {
            println!("{changes}");

            if let Some(deleted) = deleted_traces {
                println!("{deleted}");
            }
        }

        mantra::cmd::coverage::coverage_from_defmt_frames(&defmt_frames, &db, &binary_str)
            .await
            .map_err(|err| RunnerError::Mantra(err.to_string()))?;
    }

    if let Some(post_command) = &main_cfg.runner_cfg.post_runner {
        println!("--------------- Post Runner --------------------");
        let mut args = post_command.args.clone();
        args.push(binary_str);

        let output = std::process::Command::new(&post_command.name)
            .args(args)
            .current_dir(&main_cfg.workspace_dir)
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
    workspace_dir: &Path,
    tmp_gdb_file: &Path,
    runner_cfg: &RunnerConfig,
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
        .current_dir(workspace_dir)
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
    let workspace_root = workspace_dir.to_path_buf();
    let defmt_thread = std::thread::spawn(move || {
        defmt::read_defmt_frames(&binary, &workspace_root, rtt_port, thread_signal)
    });

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

/// Converts the given path into an cleaned absolute path.
/// see: https://stackoverflow.com/questions/30511331/getting-the-absolute-path-from-a-pathbuf
pub fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    }
    .clean();

    Ok(absolute_path)
}

fn mantra_db_url(url: Option<String>, embedded_dir: &Path) -> String {
    url.unwrap_or(format!(
        "sqlite://{}mantra.db?mode=rwc",
        embedded_dir.display()
    ))
}
