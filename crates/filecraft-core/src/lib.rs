mod inspect;
mod process;
mod recipe;

pub use inspect::{Inspection, inspect};
pub use process::{BatchReport, Control, FileReport, Phase, Progress, Status, run};
pub use recipe::{Capability, Format, Recipe, capabilities};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Goal(String),
    #[error("Cancelled")]
    Cancelled,
    #[error("Processing time budget exceeded")]
    Timeout,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Image(#[from] image::ImageError),
    #[error("Invalid recipe JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) | Self::Json(_) => "invalid_input",
            Self::Unsupported(_) => "unsupported",
            Self::Goal(_) => "goal_unmet",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Io(_) => "io",
            Self::Image(_) => "image",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
