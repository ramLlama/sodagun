use colored::Colorize;

use crate::context::{Context, OutputFormat};

pub struct SodagunError {
    pub code: &'static str,
    pub message: String,
}

/// Print the error in the appropriate format and exit with code 1.
/// JSON errors go to stdout so `--output json` output is always parseable;
/// text errors go to stderr.
pub fn handle_error(ctx: Context, err: SodagunError) -> ! {
    match ctx.output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({"status": "error", "code": err.code})
            );
        }
        OutputFormat::Text => {
            eprintln!("{} [{}]: {}", "Error".red().bold(), err.code, err.message);
        }
    }
    std::process::exit(1);
}
