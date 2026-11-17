use hession_core::reader::ReadError;
use serde::Serialize;
#[derive(Debug, thiserror::Error, Serialize)]
#[error("{message}")]
pub struct CliError {
    pub code: i32,
    pub kind: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
}
impl CliError {
    pub fn new(code: i32, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
            position: None,
        }
    }
}
impl From<ReadError> for CliError {
    fn from(error: ReadError) -> Self {
        let (code, kind) = match &error {
            ReadError::OutsideScope | ReadError::ScopeChanged => (4, "scope"),
            ReadError::Invalid(_) => (2, "invalid_request"),
            ReadError::InvalidCursor
            | ReadError::Search(hession_core::search::SearchError::InvalidCursor) => {
                (2, "invalid_cursor")
            }
            ReadError::Search(hession_core::search::SearchError::Invalid(_)) => {
                (2, "invalid_request")
            }
            ReadError::Project(hession_core::projects::ProjectError::OutsideScope) => (4, "scope"),
            ReadError::Project(hession_core::projects::ProjectError::InvalidPath) => {
                (2, "invalid_request")
            }
            ReadError::StaleCursor => (5, "stale_cursor"),
            ReadError::DataTooLarge
            | ReadError::ResponseTooLarge
            | ReadError::Search(hession_core::search::SearchError::InputTooLarge)
            | ReadError::Search(hession_core::search::SearchError::ResponseTooLarge) => {
                (5, "response_limit")
            }
            ReadError::Interrupted => (6, "interrupted"),
            ReadError::Search(hession_core::search::SearchError::Syntax(_)) => (2, "syntax"),
            ReadError::Search(hession_core::search::SearchError::StaleCursor) => {
                (5, "stale_cursor")
            }
            ReadError::Search(hession_core::search::SearchError::TimedOut) => (6, "timeout"),
            _ => (3, "library"),
        };
        let position = match &error {
            ReadError::Search(hession_core::search::SearchError::Syntax(error)) => {
                Some(error.position)
            }
            _ => None,
        };
        let mut result = Self::new(code, kind, error.to_string());
        result.position = position;
        result
    }
}
impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::new(3, "io", error.to_string())
    }
}
impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(2, "json", error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_search_input_limits_preserve_the_stable_size_exit_category() {
        let error = CliError::from(ReadError::Search(
            hession_core::search::SearchError::InputTooLarge,
        ));
        assert_eq!(error.code, 5);
        assert_eq!(error.kind, "response_limit");
    }
}
