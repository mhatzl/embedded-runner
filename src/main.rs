use clap::Parser;
use embedded_runner as _;

#[tokio::main]
async fn main() {
    env_logger::init();

    let cfg = embedded_runner::cfg::CliConfig::parse();

    match embedded_runner::run(cfg).await {
        Ok(status) => {
            if !status.success() {
                log::error!("GDB did not run successfully. exit code: '{status}'");
            }
        }
        Err(err) => {
            log::error!("Embedded runner failed: {err}");
        }
    }

    // println!("Starting GDB");

    // let mut gdb_cmd = Command::new("arm-none-eabi-gdb");
    // let mut gdb = gdb_cmd
    //     .args(["-x", "embedded.gdb", &args.binary])
    //     .stdout(Stdio::piped())
    //     .stderr(Stdio::piped())
    //     .spawn()
    //     .unwrap();

    // let mut open_ocd_output = gdb.stderr.take().unwrap();

    // let mut buf = [0; 100];
    // let mut content = Vec::new();
    // let rtt_start = b"for rtt connection";

    // let start = std::time::Instant::now();
    // 'outer: while let Ok(n) = open_ocd_output.read(&mut buf) {
    //     if n > 0 {
    //         content.extend_from_slice(&buf[..n]);

    //         print!("{}", String::from_utf8_lossy(&buf[..n]));

    //         for i in 0..n {
    //             let slice_end = content.len().saturating_sub(i);
    //             if content[..slice_end].ends_with(rtt_start) {
    //                 break 'outer;
    //             }
    //         }
    //     } else if std::time::Instant::now()
    //         .checked_duration_since(start)
    //         .unwrap()
    //         .as_millis()
    //         > 12000
    //     {
    //         println!("TIMEOUT");
    //         let _ = gdb.kill();
    //         exit(1);
    //     }
    // }

    // let mut defmt_print = Command::new("defmt-print")
    //     .args(["-e", &args.binary, "tcp"])
    //     .stdout(Stdio::piped())
    //     .spawn()
    //     .unwrap();
    // let mut defmt_output = defmt_print.stdout.take().unwrap();

    // let gdb_status = gdb.wait().unwrap();

    // println!("Closing GDB");
    // std::thread::sleep(std::time::Duration::from_millis(100));

    // let _ = defmt_print.kill();

    // let mut logged_content = Vec::new();
    // defmt_output.read_to_end(&mut logged_content).unwrap();

    // println!("------------ Logs ---------------");
    // println!("{}", String::from_utf8_lossy(&logged_content));

    // #[cfg(feature = "mantra")]
    // {
    //     let reqs = mantra_rust_macros::extract::extract_covered_reqs(&logged_content).unwrap();

    //     dbg!(reqs);
    // }

    // exit(gdb_status.code().unwrap());
}
