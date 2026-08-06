use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    ToolUnavailable(String),
    #[error("{0}")]
    Build(String),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("could not encode build data: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("task failed: {0}")]
    Task(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalidInput",
            Self::NotFound(_) => "notFound",
            Self::ToolUnavailable(_) => "toolUnavailable",
            Self::Build(_) => "buildFailed",
            Self::Io(_) => "filesystem",
            Self::Database(_) => "database",
            Self::Encoding(_) => "encoding",
            Self::Task(_) => "task",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorPayload<'a> {
    code: &'static str,
    message: &'a str,
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let message = self.to_string();
        ErrorPayload {
            code: self.code(),
            message: &message,
        }
        .serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
