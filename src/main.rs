//! AWS Costs TUI - Terminal UI for viewing AWS Cost Explorer data
//!
//! A beautiful terminal interface to view your AWS costs broken down by service,
//! with colorful charts and trend visualization.

mod aws;
mod ui;

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// AWS Costs TUI - View your AWS costs in the terminal
#[derive(Parser, Debug)]
#[command(name = "aws-costs")]
#[command(author = "Ajay")]
#[command(version = "0.1.0")]
#[command(about = "Terminal UI for viewing AWS Cost Explorer data with charts", long_about = None)]
struct Args {
    /// AWS profile to use (defaults to 'default' or AWS_PROFILE env var)
    #[arg(short, long, env = "AWS_PROFILE", default_value = "default")]
    profile: String,

    /// AWS region (defaults to profile region, AWS_REGION, or us-east-1)
    #[arg(short, long, env = "AWS_REGION")]
    region: Option<String>,

    /// Enable debug logging (logs to stderr)
    #[arg(long, default_value = "false")]
    debug: bool,

    /// Just print costs without TUI (useful for scripts)
    #[arg(long, default_value = "false")]
    no_tui: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Setup logging
    let filter = if args.debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("warn")
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .init();

    info!("Starting AWS Costs TUI");
    info!("Using profile: {}", args.profile);

    // Load credentials
    let credentials = aws::Credentials::load(&args.profile, args.region.as_deref())?;
    info!("Loaded credentials for region: {}", credentials.region);

    // Create Cost Explorer client
    let client = aws::CostExplorerClient::new(credentials);

    if args.no_tui {
        // Simple text output mode
        run_text_mode(&client)?;
    } else {
        // TUI mode
        run_tui_mode(&client)?;
    }

    Ok(())
}

fn run_tui_mode(client: &aws::CostExplorerClient) -> Result<()> {
    let mut app = ui::App::new();
    
    // Load data before starting TUI
    app.load_data(client);
    
    // Run the TUI
    app.run()
}

fn run_text_mode(client: &aws::CostExplorerClient) -> Result<()> {
    println!("☁️  AWS Cost Explorer\n");

    // Get current month costs
    match client.get_current_month_costs() {
        Ok(data) => {
            println!("📅 {}", data.period);
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!(
                "💰 Total: ${:.2} {}\n",
                data.total_cost, data.currency
            );

            println!("📋 Service Breakdown:");
            println!("{:<40} {:>12} {:>8}", "Service", "Cost", "%");
            println!("{}", "─".repeat(62));

            for service in &data.breakdown {
                let name = truncate(&service.service, 38);
                println!(
                    "{:<40} {:>10.2} {:>7.1}%",
                    name, service.cost, service.percentage
                );
            }
        }
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            eprintln!("\nMake sure you have:");
            eprintln!("  1. Valid AWS credentials configured");
            eprintln!("  2. Cost Explorer API access enabled");
            eprintln!("  3. ce:GetCostAndUsage permission");
            return Err(e);
        }
    }

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    let s = s
        .trim_start_matches("Amazon ")
        .trim_start_matches("AWS ")
        .trim_start_matches("Amazon");
    
    if s.len() > max_len {
        format!("{}...", &s[..max_len - 3])
    } else {
        s.to_string()
    }
}
