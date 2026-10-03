#[tokio::main]
async fn main() {
    if let Err(error) = antigravity_tools_lib::runtime::run().await {
        eprintln!("server startup failed: {error}");
        std::process::exit(1);
    }
}
