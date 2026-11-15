#![forbid(unsafe_code)]

#[path = "operations/backend.rs"]
mod backend;
#[path = "operations/fixture.rs"]
mod fixture;
#[path = "operations/model.rs"]
mod model;
#[path = "operations/recovery.rs"]
mod recovery;

use std::process::ExitCode;

#[tokio::main(worker_threads = 2)]
async fn main() -> ExitCode {
    let executable = std::env::current_exe().unwrap();

    let role = match executable.file_stem().and_then(|name| name.to_str()) {
        Some("krabb-plugin-runner") => Some("runner".to_owned()),
        Some("krabb-plugin-fixture") => Some("fixture".to_owned()),
        _ => std::env::var("KRABB_OPERATION_ROLE").ok(),
    };

    match role.as_deref() {
        Some("runner") => {
            fixture::record_adapter();
            return krabb_log::run_plugin("runner", krabb_plugin_runner::serve()).await;
        }

        Some("fixture") => return krabb_log::run_plugin("fixture", model::serve()).await,

        Some("cli") => return krabb_runtime::cli().await,

        Some("host") => {
            krabb_runtime::daemon().await.unwrap();
            return ExitCode::SUCCESS;
        }

        Some("backend") => {
            backend::serve().await;
            return ExitCode::SUCCESS;
        }

        _ => {}
    }

    let tests = [
        "operations::saved_handle_survives_host_adapter_and_backend_restart",
        "operations::lost_start_receipt_never_replays_the_external_effect",
        "operations::lost_cancel_receipt_is_reconciled_without_another_cancel",
        "operations::missing_card_is_rejected_before_operation_intent",
    ];

    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--list") {
        for test in tests {
            println!("{test}: test");
        }

        return ExitCode::SUCCESS;
    }

    if args.iter().any(|arg| arg == "--ignored") {
        return ExitCode::SUCCESS;
    }

    let mut filters = Vec::new();
    let mut skips = Vec::new();
    let mut options = args.iter();

    while let Some(arg) = options.next() {
        if arg == "--skip" {
            skips.push(options.next().expect("--skip requires a test name"));
        } else if matches!(arg.as_str(), "--test-threads" | "--format" | "--color") {
            options.next();
        } else if !arg.starts_with('-') {
            filters.push(arg.as_str());
        }
    }

    for (index, test) in tests.iter().enumerate() {
        let matches = |filter: &str| {
            if args.iter().any(|arg| arg == "--exact") {
                *test == filter
            } else {
                test.contains(filter)
            }
        };

        if !filters.is_empty() && !filters.iter().any(|filter| matches(filter))
            || skips.iter().any(|filter| matches(filter))
        {
            continue;
        }

        print!("test {test} ... ");

        match index {
            0 => recovery::saved_handle().await,
            1 => recovery::lost_start().await,
            2 => recovery::lost_cancel().await,
            _ => recovery::missing_card().await,
        }

        println!("ok");
    }

    ExitCode::SUCCESS
}
