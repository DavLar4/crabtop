/// error.rs — Application error types

use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum BtopError {
    #[error("Terminal error: {0}")]
    Terminal(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Collection error: {0}")]
    Collect(String),

    #[error("Privilege error: {0}")]
    Privilege(String),
}
