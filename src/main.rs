/* src/main.rs */

use dotenvy::dotenv;
use fancy_log::{LogLevel, log, set_log_level};
use std::env;

#[tokio::main]
async fn main() {
    dotenv().ok();
    let level = env::var("LOG_LEVEL")
        .unwrap_or_else(|_| "info".to_string())
        .to_lowercase();
    let log_level = match level.as_str() {
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    set_log_level(log_level);
    log(LogLevel::Info, "Hello World!");
}
