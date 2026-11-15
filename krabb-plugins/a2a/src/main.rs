#![forbid(unsafe_code)]

#[tokio::main]
async fn main() -> std::process::ExitCode {
    krabb_log::run_plugin("a2a", krabb_plugin_a2a::serve()).await
}
