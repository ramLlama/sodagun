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
}
