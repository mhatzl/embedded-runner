use std::{
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::AtomicBool, Arc},
};

use cfg::{CliConfig, ResolvedConfig, RunCmdConfig, RunnerConfig};
use coverage::CoverageError;
use defmt_json_schema::v1::JsonFrame;
use path_clean::PathClean;
use serde_json::json;

pub mod cfg;
pub mod coverage;
pub mod defmt;
pub mod path;

pub const DEFAULT_RTT_PORT: u16 = 19021;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Timeout waiting for rtt connection to start.")]
    RttTimeout,
    #[error("Error from gdb: {}", .0)]
    Gdb(String),
    #[error("Error setting up the gdb script: {}", .0)]
    GdbScript(String),
    // #[error("Error reading the runner config: {}", .0.display())]
    // ReadingCfg(PathBuf),
    // #[error("Error parsing the runner config: {}", .0)]
    // ParsingCfg(String),
    #[error("{}", .0)]
    Config(#[from] cfg::ConfigError),
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
    #[error("Could not create coverage data. Cause: {}", .0)]
    Coverage(CoverageError),
}

pub async fn run(cli_cfg: CliConfig) -> Result<(), RunnerError> {
    let cfg = cfg::get_cfg(&cli_cfg)?;

    match cli_cfg.cmd {
        cfg::Cmd::Run(run_cfg) => run_cmd(&cfg, run_cfg).await,
        // cfg::Cmd::Mantra(mantra_cmd) => {
        //     let mantra_cfg = mantra::cfg::Config {
        //         db: mantra::db::Config {
        //             url: Some(mantra_db_url(
        //                 cfg.runner_cfg.mantra.and_then(|m| m.db_url),
        //                 &cfg.embedded_dir,
        //             )),
        //         },
        //         cmd: mantra_cmd.clone(),
        //     };

        //     mantra::run(mantra_cfg)
        //         .await
        //         .map_err(|err| RunnerError::Mantra(err.to_string()))
        // }
    }
}

pub async fn run_cmd(main_cfg: &ResolvedConfig, run_cfg: RunCmdConfig) -> Result<(), RunnerError> {
    let output_dir = match run_cfg.output_dir {
        Some(dir) => dir,
        None => {
            let mut dir = run_cfg.binary.clone();
            dir.set_file_name(format!(
                "{}_runner",
                run_cfg
                    .binary
                    .file_name()
                    .expect("Binary name must be a valid filename.")
                    .to_string_lossy()
            ));
            dir
        }
    };

    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir).map_err(|err| {
            RunnerError::Setup(format!(
                "Could not create directory '{}'. Cause: {}",
                output_dir.display(),
                err
            ))
        })?;
    }

    let log_filepath = output_dir.join("defmt.log");
    let rel_log_filepath = log_filepath
        .strip_prefix(
            crate::path::get_cargo_root().unwrap_or(std::env::current_dir().unwrap_or_default()),
        )
        .map(|p| p.to_path_buf())
        .unwrap_or(log_filepath.clone());

    let binary_str = run_cfg.binary.display().to_string();
    let rel_binary_path = run_cfg
        .binary
        .strip_prefix(
            crate::path::get_cargo_root().unwrap_or(std::env::current_dir().unwrap_or_default()),
        )
        .map(|p| p.to_path_buf())
        .unwrap_or(run_cfg.binary.clone());
    let rel_binary_str = rel_binary_path.display().to_string();

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
        .gdb_script(&run_cfg.binary, &output_dir)
        .map_err(|_err| RunnerError::GdbScript(String::new()))?;

    let gdb_script_file = output_dir.join("embedded.gdb");
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
    let log_file = std::fs::File::create(&log_filepath).map_err(|err| {
        RunnerError::Setup(format!(
            "Could not create file '{}'. Cause: {}",
            log_filepath.display(),
            err
        ))
    })?;
    let mut writer = BufWriter::new(&log_file);

    for frame in &defmt_frames {
        let _ = writeln!(
            &mut writer,
            "{}",
            serde_json::to_string(frame).expect("DefmtFrame is valid JSON.")
        );

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

    println!("------------------ Output ---------------");
    println!("Logs written to '{}'.", log_filepath.display());

    let run_name = run_cfg
        .run_name
        .unwrap_or(rel_binary_path.display().to_string());

    let meta_path = run_cfg
        .meta_filepath
        .unwrap_or(main_cfg.embedded_dir.join("meta.json"));

    let meta = if meta_path.exists() {
        let meta_content = std::fs::read_to_string(&meta_path).map_err(|err| {
            RunnerError::Setup(format!(
                "Could not read metadata '{}'. Cause: {}",
                meta_path.display(),
                err
            ))
        })?;

        let mut meta: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&meta_content).map_err(|err| {
                RunnerError::Setup(format!(
                    "Could not deserialize metadata '{}'. Cause: {}",
                    meta_path.display(),
                    err
                ))
            })?;

        meta.insert(
            "binary".to_string(),
            serde_json::Value::String(rel_binary_str),
        );

        serde_json::Value::Object(meta)
    } else {
        json!({
            "binary": rel_binary_str
        })
    };

    let coverage = coverage::coverage_from_defmt_frames(
        run_name,
        Some(meta),
        &defmt_frames,
        Some(rel_log_filepath),
    )
    .map_err(RunnerError::Coverage)?;

    let coverage_file = output_dir.join("coverage.json");
    std::fs::write(
        &coverage_file,
        serde_json::to_string(&coverage).expect("Coverage schema is valid JSON."),
    )
    .map_err(|err| {
        RunnerError::Setup(format!(
            "Could not write to file '{}'. Cause: {}",
            coverage_file.display(),
            err
        ))
    })?;

    println!("Coverage written to '{}'.", coverage_file.display());

    // if let Some(mantra_cfg) = &main_cfg.runner_cfg.mantra {
    //     println!("------------- Mantra -------------");

    //     let db_url = mantra_db_url(mantra_cfg.db_url.clone(), &main_cfg.embedded_dir);
    //     let db = mantra::db::MantraDb::new(&mantra::db::Config { url: Some(db_url) })
    //         .await
    //         .map_err(|err| RunnerError::Mantra(err.to_string()))?;

    //     if let Some(mut extract_cfg) = mantra_cfg.extract.clone() {
    //         extract_cfg.root = absolute_path(&extract_cfg.root)
    //             .expect("Either Cargo workspace or current directory must exist.");

    //         let req_changes = mantra::cmd::extract::extract(&db, &extract_cfg)
    //             .await
    //             .map_err(|err| RunnerError::Mantra(format!("extract: {}", err)))?;

    //         let deleted_reqs = db
    //             .delete_req_generations(req_changes.new_generation)
    //             .await
    //             .map_err(|err| RunnerError::Mantra(format!("extract: {}", err)))?;
    //         db.reset_req_generation().await;

    //         if main_cfg.verbose {
    //             println!("{req_changes}");

    //             if let Some(deleted) = deleted_reqs {
    //                 println!("{deleted}");
    //             }
    //         }
    //     }

    //     let mut changes = mantra::cmd::trace::trace(
    //         &db,
    //         &mantra::cmd::trace::Config {
    //             root: main_cfg.workspace_dir.clone(),
    //             keep_root_absolute: false,
    //         },
    //     )
    //     .await
    //     .map_err(|err| RunnerError::Mantra(format!("trace: {}", err)))?;

    //     let first_generation = changes.new_generation;

    //     if let Some(extern_traces) = &mantra_cfg.extern_traces {
    //         for trace_root in extern_traces {
    //             match absolute_path(trace_root) {
    //                 Ok(abs_path) => {
    //                     let mut extern_changes = mantra::cmd::trace::trace(
    //                         &db,
    //                         &mantra::cmd::trace::Config {
    //                             root: abs_path,
    //                             keep_root_absolute: true,
    //                         },
    //                     )
    //                     .await
    //                     .map_err(|err| RunnerError::Mantra(format!("trace: {}", err)))?;

    //                     changes.merge(&mut extern_changes);
    //                 }
    //                 Err(_) => {
    //                     log::error!("Skipped bad extern trace root '{}'.", trace_root.display());
    //                 }
    //             }
    //         }
    //     }

    //     let deleted_traces = db
    //         .delete_trace_generations(first_generation)
    //         .await
    //         .map_err(|err| RunnerError::Mantra(format!("trace: {}", err)))?;
    //     db.reset_trace_generation().await;

    //     if main_cfg.verbose {
    //         println!("{changes}");

    //         if let Some(deleted) = deleted_traces {
    //             println!("{deleted}");
    //         }
    //     }

    //     let test_run_name = rel_binary_path.display().to_string();
    //     mantra::cmd::coverage::coverage_from_defmt_frames(&defmt_frames, &db, &test_run_name)
    //         .await
    //         .map_err(|err| RunnerError::Mantra(format!("coverage: {}", err)))?;

    //     println!("Updated mantra.");
    // }

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
    let start = std::time::Instant::now();
    let gdb_result;

    loop {
        match gdb.try_wait() {
            Ok(Some(exit_code)) => {
                gdb_result = Ok(exit_code);
                break;
            }
            Ok(None) => {
                if std::time::Instant::now()
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
            Err(err) => {
                gdb_result = Err(RunnerError::Gdb(format!(
                    "Error waiting for gdb to finish. Cause: {err}"
                )));
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

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

/// Converts the given path into a cleaned absolute path.
/// see: https://stackoverflow.com/questions/30511331/getting-the-absolute-path-from-a-pathbuf
pub fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate::path::get_cargo_root()
            .or_else(|_| std::env::current_dir())?
            .join(path)
    }
    .clean();

    Ok(absolute_path)
}

// fn mantra_db_url(url: Option<String>, embedded_dir: &Path) -> String {
//     url.unwrap_or(format!(
//         "sqlite://{}mantra.db?mode=rwc",
//         embedded_dir.display()
//     ))
// }
