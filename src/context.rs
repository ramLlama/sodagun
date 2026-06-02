use clap::ValueEnum;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub output: OutputFormat,
    pub quiet: bool,
}

impl Context {
    /// Print an informational line to stderr; suppressed when `--quiet`.
    pub fn log(&self, msg: &str) {
        if !self.quiet {
            eprintln!("{}", msg);
        }
    }
}
