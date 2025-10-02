use std::{
    io::Read,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use defmt_decoder::{DecodeError, Frame, Locations, Table};
use defmt_json_schema::v1::{JsonFrame, Location as JsonLocation, ModulePath};

#[derive(Debug, thiserror::Error)]
pub enum DefmtError {
    #[error("Received a malformend frame.")]
    MalformedFrame,
    #[error("No frames received.")]
    NoFramesReceived,
    #[error("TCP error: {}", .0)]
    TcpError(String),
    #[error("TCP connection error: {}", .0)]
    TcpConnect(String),
    #[error("Failed reading binary. Cause: {}", .0)]
    ReadBinary(std::io::Error),
    #[error("Missing defmt data in given binary.")]
    MissingDefmt,
}

pub fn read_defmt_frames(
    binary: &Path,
    workspace_root: &Path,
    mut stream: std::net::TcpStream,
    end_signal: Arc<AtomicBool>,
) -> Result<Vec<JsonFrame>, DefmtError> {
    let bytes = std::fs::read(binary).map_err(DefmtError::ReadBinary)?;
    let table = Table::parse(&bytes)
        .map_err(|_| DefmtError::MissingDefmt)?
        .ok_or(DefmtError::MissingDefmt)?;
    let locs = table
        .get_locations(&bytes)
        .map_err(|_| DefmtError::MissingDefmt)?;

    // check if the locations info contains all the indicies
    let locs = if table.indices().all(|idx| locs.contains_key(&(idx as u64))) {
        Some(locs)
    } else {
        log::warn!("(BUG) location info is incomplete; it will be omitted from the output");
        None
    };

    const READ_BUFFER_SIZE: usize = 1024;
    let mut buf = [0; READ_BUFFER_SIZE];
    let mut decoder = table.new_stream_decoder();
    let mut stream_decoder = Box::pin(&mut decoder);

    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut json_frames = Vec::new();

    loop {
        // read from tcpstream and push it to the decoder
        if end_signal.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(json_frames);
        }

        let n = match stream.read(&mut buf) {
            Ok(len) => {
                if len == 0 {
                    continue;
                } else {
                    len
                }
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::TimedOut {
                    continue;
                } else if matches!(
                    err.kind(),
                    std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                ) {
                    return Ok(json_frames);
                } else {
                    return Err(DefmtError::TcpError(err.to_string()));
                }
            }
        };

        stream_decoder.received(&buf[..n]);

        // decode the received data
        loop {
            match stream_decoder.decode() {
                Ok(frame) => {
                    let json_frame = create_json_frame(workspace_root, &frame, &locs);

                    let mod_path = if let Some(mod_path) = &json_frame.location.module_path {
                        if mod_path.modules.is_empty() {
                            Some(format!("{}::{}", mod_path.crate_name, mod_path.function))
                        } else {
                            Some(format!(
                                "{}::{}::{}",
                                mod_path.crate_name,
                                mod_path.modules.join("::"),
                                mod_path.function
                            ))
                        }
                    } else {
                        None
                    };

                    // use kv feature due to lifetime problems with arg
                    let val = Some([("msg", log::kv::Value::from_display(&json_frame.data))]);

                    match json_frame.level {
                        Some(level) => {
                            let log_record = log::RecordBuilder::new()
                                .level(level)
                                .file(json_frame.location.file.as_deref())
                                .line(json_frame.location.line)
                                .module_path(mod_path.as_deref())
                                .target("embedded")
                                .key_values(&val)
                                .build();
                            log::logger().log(&log_record);
                        }
                        None => {
                            // mantra coverage logs not printed to remove clutter
                            if mantra_rust_macros::extract::extract_first_coverage(&json_frame.data)
                                .is_none()
                            {
                                println!("TARGET-PRINT | {}", json_frame.data);

                                if log::Level::Trace <= log::STATIC_MAX_LEVEL
                                    && log::Level::Trace <= log::max_level()
                                {
                                    let location = if json_frame.location.file.is_some()
                                        && json_frame.location.line.is_some()
                                        && mod_path.is_some()
                                    {
                                        format!(
                                            "{} in {}:{}",
                                            mod_path.unwrap(),
                                            json_frame.location.file.as_ref().unwrap(),
                                            json_frame.location.line.unwrap(),
                                        )
                                    } else {
                                        "no-location info available".to_string()
                                    };

                                    println!("             | => {location}");
                                }
                            }
                        }
                    }

                    json_frames.push(json_frame);
                }
                Err(DecodeError::UnexpectedEof) => break,
                Err(DecodeError::Malformed) => match table.encoding().can_recover() {
                    // if recovery is impossible, abort
                    false => return Err(DefmtError::MalformedFrame),
                    // if recovery is possible, skip the current frame and continue with new data
                    true => {
                        log::warn!("Malformed defmt frame skipped!");
                        continue;
                    }
                },
            }
        }
    }
}

pub type LocationInfo = (Option<String>, Option<u32>, Option<String>);

pub fn location_info(
    workspace_root: &Path,
    frame: &Frame,
    locs: &Option<Locations>,
) -> LocationInfo {
    let (mut file, mut line, mut mod_path) = (None, None, None);

    let loc = locs.as_ref().map(|locs| locs.get(&frame.index()));

    if let Some(Some(loc)) = loc {
        // try to get the relative path from workspace root, else the full one
        let path = mantra_lang_tracing::path::make_relative(&loc.file, workspace_root)
            .unwrap_or(loc.file.to_path_buf());

        file = Some(path.display().to_string());
        line = Some(loc.line as u32);
        mod_path = Some(loc.module.clone());
    }

    (file, line, mod_path)
}

/// Create a new [JsonFrame] from a log-frame from the target
pub fn create_json_frame(
    workspace_root: &Path,
    frame: &Frame,
    locs: &Option<Locations>,
) -> JsonFrame {
    let (file, line, mod_path) = location_info(workspace_root, frame, locs);
    let host_timestamp = time::OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .min(i64::MAX as i128) as i64;

    JsonFrame {
        data: frame.display_message().to_string(),
        host_timestamp,
        level: frame.level().map(to_json_level),
        location: JsonLocation {
            file,
            line,
            module_path: create_module_path(mod_path.as_deref()),
        },
        target_timestamp: frame
            .display_timestamp()
            .map(|ts| ts.to_string())
            .unwrap_or_default(),
    }
}

fn create_module_path(module_path: Option<&str>) -> Option<ModulePath> {
    let mut path = module_path?.split("::").collect::<Vec<_>>();

    // there need to be at least two elements, the crate and the function
    if path.len() < 2 {
        return None;
    };

    // the last element is the function
    let function = path.pop()?.to_string();
    // the first element is the crate_name
    let crate_name = path.remove(0).to_string();

    Some(ModulePath {
        crate_name,
        modules: path.into_iter().map(|a| a.to_string()).collect(),
        function,
    })
}

pub fn to_json_level(level: defmt_parser::Level) -> log::Level {
    match level {
        defmt_parser::Level::Trace => log::Level::Trace,
        defmt_parser::Level::Debug => log::Level::Debug,
        defmt_parser::Level::Info => log::Level::Info,
        defmt_parser::Level::Warn => log::Level::Warn,
        defmt_parser::Level::Error => log::Level::Error,
    }
}
