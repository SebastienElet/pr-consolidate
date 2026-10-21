use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Refused(String),
    Failed(String),
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn code(&self) -> i32 {
        match self {
            Self::Refused(_) => 2,
            Self::Failed(_) => 1,
            Self::Timeout(_) => 3,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Refused(_) => "REFUSED",
            Self::Failed(_) => "FAILED",
            Self::Timeout(_) => "TIMEOUT",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(message) | Self::Failed(message) | Self::Timeout(message) => {
                message.fmt(f)
            }
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Refused(error.to_string())
    }
}

pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Refused(message.into()))
    }
}
