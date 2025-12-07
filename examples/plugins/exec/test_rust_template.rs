/* examples/plugins/exec/test_rust_template.rs */

use std::io::{self, BufRead, Write};

fn main() {
    // Print debug info to Stderr
    eprintln!("⚙ Starting execution...");

    // Read all stdin safely
    let stdin = io::stdin();
    let mut input_raw = String::new();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) => {
                input_raw.push_str(&l);
                input_raw.push('\n');
            }
            Err(_) => break,
        }
    }
    if input_raw.is_empty() {
        eprintln!("✗ No input received on Stdin!");
        std::process::exit(1);
    }

    // Remove trailing newline
    if input_raw.ends_with('\n') {
        input_raw.pop();
    }

    eprintln!("⚙ Received Input: {}", input_raw);

    // Parse JSON manually for {"auth_token":"..."} structure
    let auth_token = input_raw
        .split("\"auth_token\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or("");

    // Business Logic
    let (branch, store) = if auth_token == "secret123" {
        eprintln!("✓ Auth success!");
        ("success", r#"{"user_role":"admin","verified":"true"}"#)
    } else {
        eprintln!("✗ Auth failed. Token was: {}", auth_token);
        ("failure", r#"{"error_reason":"invalid_token"}"#)
    };

    // Output result to Stdout (compact JSON)
    print!("{{\"branch\":\"{}\",\"store\":{}}}", branch, store);
    io::stdout().flush().unwrap();
}
