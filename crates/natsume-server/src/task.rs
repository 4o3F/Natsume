use snafu::Snafu;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum TaskFailure {
    #[snafu(display("the task was cancelled"))]
    Cancelled,
    #[snafu(display("the task panicked"))]
    Panicked,
}

impl From<tokio::task::JoinError> for TaskFailure {
    fn from(error: tokio::task::JoinError) -> Self {
        if error.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Panicked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskFailure;

    #[tokio::test]
    async fn task_sources_preserve_classification_without_panic_payloads() {
        let error = tokio::spawn(async { panic!("task-source-secret-canary") })
            .await
            .err()
            .unwrap_or_else(|| panic!("panicking task completed"));
        let error = TaskFailure::from(error);
        assert_eq!(error, TaskFailure::Panicked);
        for value in [
            format!("{error:?}"),
            snafu::Report::from_error(error).to_string(),
        ] {
            assert!(!value.contains("task-source-secret-canary"));
        }
        let cancelled = tokio::spawn(std::future::pending::<()>());
        cancelled.abort();
        assert_eq!(
            TaskFailure::from(
                cancelled
                    .await
                    .err()
                    .unwrap_or_else(|| panic!("cancelled task completed"))
            ),
            TaskFailure::Cancelled
        );
    }
}
