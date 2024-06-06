use std::path::PathBuf;

use defmt_json_schema::v1::JsonFrame as DefmtFrame;
use mantra_schema::{
    coverage::{CoverageSchema, Test, TestRun, TestState},
    traces::TracePk,
};
use regex::Regex;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    #[error("{}", .0)]
    Fs(#[from] std::io::Error),
    #[error("{}", .0)]
    Deserialize(#[from] serde_json::Error),
    #[error("No tests found.")]
    NoTests,
    #[error("{}", .0)]
    BadDate(String),
    #[error("{}", .0)]
    Match(String),
}

pub fn coverage_from_defmt_frames(
    run_name: String,
    meta: Option<serde_json::Value>,
    frames: &[DefmtFrame],
    log_file: Option<PathBuf>,
) -> Result<CoverageSchema, CoverageError> {
    if frames.is_empty() {
        return Err(CoverageError::NoTests);
    }

    let timestamp = frames
        .first()
        .expect("At least one frame must be available.")
        .host_timestamp;
    let date = OffsetDateTime::from_unix_timestamp_nanos(timestamp as i128).map_err(|_| {
        CoverageError::BadDate(format!("Timestamp '{timestamp}' is not a valid date."))
    })?;

    let mut test_run = TestRun {
        name: run_name,
        date,
        meta,
        log_file,
        tests: Vec::new(),
        nr_of_tests: 0,
    };

    let mut current_test: Option<Test> = None;

    let test_fn_matcher = TEST_FN_MATCHER.get_or_init(|| {
        Regex::new(
            r"^\(\d+/(?<nr_tests>\d+)\)\s(?<state>(?:running)|(?:ignoring))\s`(?<fn_name>.+)`...",
        )
        .expect("Could not create regex matcher for defmt test-fn entries.")
    });

    for frame in frames {
        if let Some(captured_test_fn) = test_fn_matcher.captures(&frame.data) {
            if let Some(mut test) = current_test.take() {
                test.state = TestState::Passed;
                test_run.tests.push(test);
            }

            let nr_tests: u32 = captured_test_fn
                .name("nr_tests")
                .expect("Number of tests from the test-fn was not captured.")
                .as_str()
                .parse()
                .expect("Number of tests must be convertible to u32.");

            test_run.nr_of_tests = nr_tests;

            let fn_state = captured_test_fn
                .name("state")
                .expect("State of the test-fn was not captured.");
            let fn_name = captured_test_fn
                .name("fn_name")
                .expect("Name of the test-fn was not captured.");

            let Some(file) = &frame.location.file else {
                return Err(CoverageError::Match(format!(
                    "Missing file location information for log entry '{}'.",
                    frame.data
                )));
            };
            let Some(line_nr) = frame.location.line else {
                return Err(CoverageError::Match(format!(
                    "Missing line location information for log entry '{}'.",
                    frame.data
                )));
            };
            let Some(mod_path) = &frame.location.module_path else {
                return Err(CoverageError::Match(format!(
                    "Missing line location information for log entry '{}'.",
                    frame.data
                )));
            };
            let mod_path_str = format!(
                "{}{}",
                mod_path.crate_name,
                if mod_path.modules.is_empty() {
                    String::new()
                } else {
                    format!("::{}", mod_path.modules.join("::"))
                }
            );

            let test_fn_name = format!("{}::{}", mod_path_str, fn_name.as_str());

            match fn_state.as_str() {
                "running" => {
                    current_test = Some(Test { name: test_fn_name, filepath: PathBuf::from(file), line: line_nr, state: TestState::Failed, covered_traces: Vec::new(), covered_lines: Vec::new() });
                }
                "ignoring" => {
                    test_run.tests.push(Test{ name: test_fn_name, filepath: PathBuf::from(file), line: line_nr, state: TestState::Skipped { reason: None }, covered_traces: Vec::new(), covered_lines: Vec::new() });

                    current_test = None;
                }
                _ => unreachable!("Invalid state '{}' for test function '{}' in log entry '{}'. Only 'running' and 'ignoring' are allowed.", fn_state.as_str(), fn_name.as_str(), frame.data),
            }
        } else if let Some(covered_req) =
            mantra_rust_macros::extract::extract_first_coverage(&frame.data)
        {
            if let Some(test) = &mut current_test {
                test.covered_traces.push(TracePk {
                    req_id: covered_req.id,
                    filepath: covered_req.file,
                    line: covered_req.line,
                });
            }
        } else if frame.data == "all tests passed!" {
            if let Some(mut test) = current_test.take() {
                test.state = TestState::Passed;
                test_run.tests.push(test);
            }
        }
    }

    Ok(CoverageSchema {
        test_runs: vec![test_run],
    })
}

static TEST_FN_MATCHER: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
