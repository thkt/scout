use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    scout::run().await
}
