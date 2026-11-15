#![forbid(unsafe_code)]

#[tokio::main]
async fn main() -> std::process::ExitCode {
    krabb_log::run_plugin("runner", krabb_plugin_runner::serve()).await
}
