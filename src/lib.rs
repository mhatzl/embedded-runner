use std::{
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{atomic::AtomicBool, Arc},
};

use cfg::{CliConfig, ResolvedConfig, RunCmdConfig, RunnerConfig};
use covcon::cfg::DataFormat;
use coverage::CoverageError;
use defmt_json_schema::v1::JsonFrame;
use path_clean::PathClean;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};

pub mod cfg;
pub mod collect;
pub mod coverage;
pub mod defmt;
pub mod path;

pub const DEFAULT_RTT_PORT: u16 = 19021;

/// Timeout defines the maximum duration to setup RTT connection between host and target
pub const SETUP_RTT_TIMEOUT_SEC: u64 = 60;
/// Timeout defines the maximum duration of one test run
pub const EXECUTION_TIMEOUT_SEC: u64 = 3600; // 1h

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Timeout waiting for rtt connection to start.")]
    RttTimeout,
    #[error("Error from gdb: {}", .0)]
    Gdb(String),
    #[error("Error setting up the gdb script: {}", .0)]
    GdbScript(String),
    #[error("{}", .0)]
    Config(#[from] cfg::ConfigError),
    #[error("Error reading defmt logs: {}", .0)]
    Defmt(String),
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
    match cli_cfg.cmd {
        cfg::Cmd::Run(run_cfg) => {
            let cfg = cfg::get_cfg(&run_cfg.runner_cfg, cli_cfg.verbose)?;
            run_cmd(&cfg, run_cfg).await
        }
        cfg::Cmd::Collect(collect_cfg) => collect::run(collect_cfg).await,
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
        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|err| {
                RunnerError::Setup(format!(
                    "Could not create directory '{}'. Cause: {}",
                    output_dir.display(),
                    err
                ))
            })?;
    }

    let log_filepath = output_dir.join("defmt.log");

    let binary_str = run_cfg.binary.display().to_string();
    let rel_binary_path = run_cfg
        .binary
        .strip_prefix(
            crate::path::get_cargo_root().unwrap_or(std::env::current_dir().unwrap_or_default()),
        )
        .map(|p| p.to_path_buf())
        .unwrap_or(run_cfg.binary.clone());
    let rel_binary_str = rel_binary_path.display().to_string();

    #[cfg(target_os = "windows")]
    let pre_command = main_cfg
        .runner_cfg
        .pre_runner_windows
        .as_ref()
        .or(main_cfg.runner_cfg.pre_runner.as_ref());
    #[cfg(not(target_os = "windows"))]
    let pre_command = main_cfg.runner_cfg.pre_runner.as_ref();

    if let Some(pre_command) = pre_command {
        println!("-------------------- Pre Runner --------------------");
        let mut args = pre_command.args.clone();
        args.push(binary_str.clone());

        let output = tokio::process::Command::new(&pre_command.name)
            .args(args)
            .current_dir(&main_cfg.workspace_dir)
            .output()
            .await
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
        .gdb_script(
            &run_cfg.binary,
            &output_dir,
            run_cfg.segger_gdb.unwrap_or(main_cfg.runner_cfg.segger_gdb),
        )
        .map_err(|_err| RunnerError::GdbScript(String::new()))?;

    let gdb_script_file = output_dir.join("embedded.gdb");
    tokio::fs::write(&gdb_script_file, gdb_script)
        .await
        .map_err(|err| RunnerError::GdbScript(err.to_string()))?;

    let (defmt_frames, gdb_result) = run_gdb_sequence(
        run_cfg.binary,
        &main_cfg.workspace_dir,
        &gdb_script_file,
        &main_cfg.runner_cfg,
    )
    .await?;
    let gdb_status = gdb_result?;

    if !gdb_status.success() {
        return Err(RunnerError::Gdb(format!(
            "GDB did not run successfully. Exit code: '{gdb_status}'"
        )));
    }

    println!("------------------ Output ------------------");

    if defmt_frames.is_empty() {
        println!("No logs received.");
    } else {
        let log_file = tokio::fs::File::create(&log_filepath)
            .await
            .map_err(|err| {
                RunnerError::Setup(format!(
                    "Could not create file '{}'. Cause: {}",
                    log_filepath.display(),
                    err
                ))
            })?;
        let mut writer = BufWriter::new(log_file);

        for frame in &defmt_frames {
            let _w = writer
                .write_all(
                    serde_json::to_string(frame)
                        .expect("DefmtFrame is valid JSON.")
                        .as_bytes(),
                )
                .await;
            let _w = writer.write_all("\n".as_bytes()).await;
        }

        let _f = writer.flush().await;

        println!("Logs written to '{}'.", log_filepath.display());

        let run_name = run_cfg
            .run_name
            .unwrap_or(rel_binary_path.display().to_string());

        let meta_path = run_cfg
            .meta_filepath
            .unwrap_or(main_cfg.embedded_dir.join("meta.json"));

        let mut meta = if meta_path.exists() {
            let meta_content = tokio::fs::read_to_string(&meta_path).await.map_err(|err| {
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

        if let Some(extern_cov) = &main_cfg.runner_cfg.extern_coverage {
            match (tokio::fs::read_to_string(&extern_cov.filepath).await, covcon::cfg::DataFormat::try_from(extern_cov.filepath.extension())) {
                (Ok(content), Ok(DataFormat::Xml)) => {
                    let cov_cfg = covcon::cfg::ConversionConfig {
                        in_fmt: extern_cov.format,
                        in_content: content,
                        in_data_fmt: DataFormat::Xml,
                        out_fmt: covcon::format::CoverageFormat::CoberturaV4,
                        out_data_fmt: DataFormat::Json,
                    };

                    match covcon::convert::convert_to_json(&cov_cfg) {
                        Ok(json_cov) => {
                            let meta_map = meta.as_object_mut().expect("Meta is created as object above.");
                            meta_map.insert("coverage".to_string(), json_cov);
                        },
                        Err(err) => log::error!("Failed extracting external coverage data. External coverage will be ignored. Cause: {err}"),
                    }
                }
                (Err(err), _) => log::error!("Failed to read external coverage file. External coverage will be ignored. Cause: {err}"),
                (_, _) => log::error!("Coverage file must be XML. External coverage will be ignored."),
            }
        }

        let logs =
            serde_json::to_string(&defmt_frames).expect("DefmtFrames were deserialized before.");

        let coverage =
            coverage::coverage_from_defmt_frames(run_name, Some(meta), &defmt_frames, Some(logs))
                .map_err(RunnerError::Coverage)?;

        // If no tests were found, execution most likely `cargo run` or `cargo bench` => no test coverage
        if coverage
            .test_runs
            .iter()
            .any(|test_run| test_run.nr_of_tests > 0)
        {
            let coverage_file = output_dir.join("coverage.json");
            tokio::fs::write(
                &coverage_file,
                serde_json::to_string(&coverage).expect("Coverage schema is valid JSON."),
            )
            .await
            .map_err(|err| {
                RunnerError::Setup(format!(
                    "Could not write to file '{}'. Cause: {}",
                    coverage_file.display(),
                    err
                ))
            })?;

            println!("Coverage written to '{}'.", coverage_file.display());

            let coverages_filepath = coverage::coverages_filepath();

            if !coverages_filepath.exists() {
                let _w =
                    tokio::fs::write(coverages_filepath, coverage_file.display().to_string()).await;
            } else {
                let mut file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .read(true)
                    .open(coverages_filepath)
                    .await
                    .expect("Coverages file exists.");

                let mut content = String::new();
                file.read_to_string(&mut content)
                    .await
                    .expect("Reading coverages");

                let mut exists = false;
                for line in content.lines() {
                    if line == coverage_file.display().to_string() {
                        exists = true;
                        break;
                    }
                }

                if !exists {
                    let _w = file.write_all("\n".as_bytes()).await;
                    let _w = file
                        .write_all(coverage_file.display().to_string().as_bytes())
                        .await;
                }

                let _f = file.flush().await;
            }
        }
    }

    #[cfg(target_os = "windows")]
    let post_command = main_cfg
        .runner_cfg
        .post_runner_windows
        .as_ref()
        .or(main_cfg.runner_cfg.post_runner.as_ref());
    #[cfg(not(target_os = "windows"))]
    let post_command = main_cfg.runner_cfg.post_runner.as_ref();

    if let Some(post_command) = post_command {
        println!("-------------------- Post Runner --------------------");
        let mut args = post_command.args.clone();
        args.push(binary_str);

        let output = tokio::process::Command::new(&post_command.name)
            .args(args)
            .current_dir(&main_cfg.workspace_dir)
            .output()
            .await
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

pub async fn run_gdb_sequence(
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
    let mut gdb_cmd = tokio::process::Command::new(
        std::env::var("GDB").unwrap_or("arm-none-eabi-gdb".to_string()),
    );
    let mut gdb = gdb_cmd
        .args([
            "-x",
            &tmp_gdb_file.to_string_lossy(),
            &binary.to_string_lossy(),
        ])
        .current_dir(workspace_dir)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    println!("-------------------- Communication Setup --------------------");

    let rtt_port = runner_cfg.rtt_port.unwrap_or(DEFAULT_RTT_PORT);
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(SETUP_RTT_TIMEOUT_SEC),
        tokio::spawn(async move {
            loop {
                match TcpStream::connect(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), rtt_port)) {
                    Ok(stream) => {
                        return Ok(stream);
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
        }),
    )
    .await;

    let stream = match stream {
        Ok(Ok(Ok(stream))) => stream,
        Ok(Ok(Err(io_err))) => {
            log::error!("Failed to connect to RTT. Cause: {io_err}");
            let _ = gdb.kill().await;
            return Err(RunnerError::RttTimeout);
        }
        _ => {
            log::error!("Timeout while trying to connect to RTT.");
            let _ = gdb.kill().await;
            return Err(RunnerError::RttTimeout);
        }
    };

    println!();
    println!("-------------------- Running --------------------");

    // start defmt thread + end-signal
    let end_signal = Arc::new(AtomicBool::new(false));
    let thread_signal = end_signal.clone();
    let workspace_root = workspace_dir.to_path_buf();
    let defmt_thread = tokio::spawn(async move {
        defmt::read_defmt_frames(&binary, &workspace_root, stream, thread_signal)
    });

    // wait for gdb to end
    let gdb_result = match tokio::time::timeout(
        std::time::Duration::from_secs(EXECUTION_TIMEOUT_SEC),
        gdb.wait(),
    )
    .await
    {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(err)) => Err(RunnerError::Gdb(format!(
            "Error waiting for gdb to finish. Cause: {err}"
        ))),
        Err(_) => {
            log::error!("Timeout while waiting for gdb to end.");
            let _ = gdb.kill().await;
            return Err(RunnerError::RttTimeout);
        }
    };

    // signal defmt end
    end_signal.store(true, std::sync::atomic::Ordering::Relaxed);

    // join defmt thread to get logs
    let defmt_result = defmt_thread
        .await
        .map_err(|_| RunnerError::Defmt("Failed waiting for defmt logs.".to_string()))?;

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
