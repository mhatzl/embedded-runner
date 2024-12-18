use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::{cfg::CollectCmdConfig, coverage, RunnerError};

pub async fn run(cfg: CollectCmdConfig) -> Result<(), RunnerError> {
    let coverages_file = coverage::coverages_filepath();
    let mut coverages = mantra_schema::coverage::CoverageSchema {
        version: Some(mantra_schema::SCHEMA_VERSION.to_string()),
        test_runs: Vec::new(),
    };

    if !coverages_file.exists() {
        log::info!("No coverage to collect.");
        return Ok(());
    }

    let files = tokio::fs::read_to_string(&coverages_file)
        .await
        .expect("Coverages file exists.");
    for line in files.lines() {
        let coverage_path = PathBuf::from(line);

        if !coverage_path.exists() {
            log::error!("Missing coverage file '{}'.", coverage_path.display());
        } else {
            let coverage_content = tokio::fs::read_to_string(coverage_path)
                .await
                .expect("Coverage file exists.");
            let mut coverage: mantra_schema::coverage::CoverageSchema =
                serde_json::from_str(&coverage_content)
                    .expect("Coverage was serialized with embedded-runner.");

            coverages.test_runs.append(&mut coverage.test_runs);
        }
    }

    if coverages.test_runs.is_empty() {
        log::info!("No coverages found.");
        return Ok(());
    }

    let output = match cfg.output {
        Some(out) => {
            if out
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                == Some("json".to_string())
            {
                out
            } else {
                return Err(RunnerError::Setup(
                    "Output path must point to a JSON file.".to_string(),
                ));
            }
        }
        None => PathBuf::from("coverage.json"),
    };

    let combined_coverage =
        serde_json::to_string(&coverages).expect("Serializing coverage schema.");

    if !output.exists() {
        let mut file = tokio::fs::File::create(output).await.map_err(|_| {
            RunnerError::Setup("Could not create combined coverage file.".to_string())
        })?;
        let _ = file.write(combined_coverage.as_bytes()).await;
    } else {
        let _ = tokio::fs::write(output, combined_coverage).await;
    }

    // To only collect the newly created coverage files
    let _ = tokio::fs::remove_file(coverages_file).await;

    Ok(())
}
