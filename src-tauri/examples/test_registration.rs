//! Test registration with 2Captcha
//!
//! Run with: cargo run --example test_registration

use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging with debug level for auth
    tracing_subscriber::fmt()
        .with_env_filter("info,app_lib::auth=debug")
        .init();

    let api_key = "9b1867140611a0e0ea985d8f2992c7d0";
    let count = 3;
    let password = "TestPass123!";

    println!("=== GrintaHub Registration Test ===\n");

    // Step 1: Check 2Captcha balance
    println!("Step 1: Checking 2Captcha balance...");
    let solver = app_lib::captcha::CaptchaSolver::new(api_key)?;

    match solver.get_balance().await {
        Ok(balance) => println!("  Balance: ${:.2}\n", balance),
        Err(e) => println!("  Warning: Could not get balance: {}\n", e),
    }

    // Step 2: Create auth client
    println!("Step 2: Creating auth client...");
    let auth_client = app_lib::auth::AuthClient::new(120)?;
    println!("  Auth client ready\n");

    // Step 3: Register accounts
    println!("Step 3: Registering {} accounts...\n", count);

    let start = Instant::now();
    let results = auth_client.batch_register(&solver, count, password, Some(3000)).await;
    let elapsed = start.elapsed();

    // Step 4: Print results
    println!("\n=== Results ===\n");

    let mut success_count = 0;
    let mut accounts = Vec::new();

    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(account) => {
                success_count += 1;
                println!("Account {}: SUCCESS", i + 1);
                println!("  Email: {}", account.email);
                println!("  Name: {}", account.name);
                println!("  Phone: {}", account.phone.as_deref().unwrap_or("N/A"));
                println!();
                accounts.push(account.clone());
            }
            Err(e) => {
                println!("Account {}: FAILED", i + 1);
                println!("  Error: {}", e);
                println!();
            }
        }
    }

    println!("=== Summary ===");
    println!("Total time: {:.1}s", elapsed.as_secs_f64());
    println!("Success: {}/{}", success_count, count);
    println!("Password for all accounts: {}", password);

    if !accounts.is_empty() {
        println!("\n=== Created Accounts ===");
        for account in &accounts {
            println!("  {} / {}", account.email, password);
        }
    }

    Ok(())
}
