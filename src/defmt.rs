use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
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
}

pub fn read_defmt_frames(
    binary: &Path,
    tcp_port: u16,
    end_signal: Arc<AtomicBool>,
) -> Result<Vec<JsonFrame>, DefmtError> {
    let bytes = std::fs::read(binary).unwrap(); //?;
    let table = Table::parse(&bytes).unwrap().unwrap(); //?.ok_or_else(|| anyhow!(".defmt data not found"))?;
    let locs = table.get_locations(&bytes).unwrap(); //?;

    // check if the locations info contains all the indicies
    let locs = if table.indices().all(|idx| locs.contains_key(&(idx as u64))) {
        Some(locs)
    } else {
        log::warn!("(BUG) location info is incomplete; it will be omitted from the output");
        None
    };

    const READ_BUFFER_SIZE: usize = 1024;
    let mut buf = [0; READ_BUFFER_SIZE];
    let mut stream_decoder = table.new_stream_decoder();

    let mut source = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), tcp_port))
        .map_err(|err| DefmtError::TcpConnect(err.to_string()))?;
    let _ = source.set_read_timeout(Some(Duration::from_secs(2)));
    let mut json_frames = Vec::new();

    loop {
        // read from tcpstream and push it to the decoder
        if end_signal.load(std::sync::atomic::Ordering::Relaxed) {
            return if json_frames.is_empty() {
                Err(DefmtError::NoFramesReceived)
            } else {
                Ok(json_frames)
            };
        }

        let n = match source.read(&mut buf) {
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
                    json_frames.push(create_json_frame(&frame, &locs));
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

pub fn location_info(frame: &Frame, locs: &Option<Locations>) -> LocationInfo {
    let (mut file, mut line, mut mod_path) = (None, None, None);

    let loc = locs.as_ref().map(|locs| locs.get(&frame.index()));

    if let Some(Some(loc)) = loc {
        // try to get the relative path, else the full one
        let current_dir = std::env::current_dir().unwrap(); //?;
        let path = loc.file.strip_prefix(current_dir).unwrap_or(&loc.file);

        file = Some(path.display().to_string());
        line = Some(loc.line as u32);
        mod_path = Some(loc.module.clone());
    }

    (file, line, mod_path)
}

/// Create a new [JsonFrame] from a log-frame from the target
pub fn create_json_frame(frame: &Frame, locs: &Option<Locations>) -> JsonFrame {
    let (file, line, mod_path) = location_info(frame, locs);
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
