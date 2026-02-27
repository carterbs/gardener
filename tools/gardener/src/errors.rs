use thiserror::Error;

#[derive(Debug, Error)]
pub enum GardenerError {
    #[error("io error: {0}")]
    Io(String),
    #[error("config parse error: {0}")]
    ConfigParse(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("cli error: {0}")]
    Cli(String),
    #[error("process error: {0}")]
    Process(String),
    #[error("output envelope error: {0}")]
    OutputEnvelope(String),
    #[error("database error: {0}")]
    Database(String),
}
