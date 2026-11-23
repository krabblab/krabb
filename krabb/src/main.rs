#![forbid(unsafe_code)]

use std::{future::Future, process::ExitCode};

#[cfg(test)]
use krabb_runtime as _;

#[tokio::main]
#[cfg(not(test))]
async fn main() -> ExitCode {
    if let Some(code) = krabb_runtime::pdf_child() {
        return code;
    }

    run(krabb_runtime::cli()).await
}

async fn run<F>(future: F) -> ExitCode
where
    F: Future<Output = ExitCode>,
{
    future.await
}

#[cfg(test)]
mod tests {
    use super::run;

    #[tokio::test]
    async fn returns_the_cli_exit_code() {
        assert_eq!(
            run(async { std::process::ExitCode::SUCCESS }).await,
            std::process::ExitCode::SUCCESS
        );

        assert_eq!(
            run(async { std::process::ExitCode::FAILURE }).await,
            std::process::ExitCode::FAILURE
        );
    }
}
