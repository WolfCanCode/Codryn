use anyhow::Result;
use std::cell::RefCell;
use std::fmt;
use std::io;

/// Error type for prompter operations.
#[derive(Debug)]
pub enum PrompterError {
    /// The user cancelled the operation (Ctrl+C or EOF on stdin).
    Cancelled,
    /// An I/O error occurred during prompting.
    Io(io::Error),
}

impl fmt::Display for PrompterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrompterError::Cancelled => write!(f, "Operation cancelled by user"),
            PrompterError::Io(e) => write!(f, "I/O error during prompt: {}", e),
        }
    }
}

impl std::error::Error for PrompterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PrompterError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl PrompterError {
    /// Returns the exit code that should be used when this error causes program termination.
    pub fn exit_code(&self) -> i32 {
        match self {
            PrompterError::Cancelled => 130,
            PrompterError::Io(_) => 1,
        }
    }
}

/// Trait for interactive prompts, enabling test mocks.
pub trait Prompter {
    /// Present options and return the selected index.
    ///
    /// `default` is the zero-based index of the pre-selected option.
    fn select(&self, prompt: &str, options: &[&str], default: usize) -> Result<usize>;

    /// Present a multi-select and return selected indices.
    ///
    /// `defaults` indicates which options are pre-selected (must be same length as `options`).
    fn multi_select(&self, prompt: &str, options: &[&str], defaults: &[bool])
        -> Result<Vec<usize>>;

    /// Ask for yes/no confirmation.
    ///
    /// `default` is the value returned if the user presses Enter without typing.
    fn confirm(&self, prompt: &str, default: bool) -> Result<bool>;

    /// Display informational text.
    fn info(&self, message: &str);

    /// Display a diff (before/after) for a given file path.
    fn show_diff(&self, path: &str, before: &str, after: &str);
}

/// Production prompter using dialoguer for interactive arrow-key navigation.
///
/// - Single-select: arrow keys to move, Enter to confirm
/// - Multi-select: arrow keys to move, Space to toggle, Enter to confirm
/// - Confirm: arrow keys or y/n, Enter to confirm
pub struct StdinPrompter;

/// Convert a dialoguer error into our PrompterError type.
fn dialoguer_error_to_prompter(err: dialoguer::Error) -> PrompterError {
    match err {
        dialoguer::Error::IO(e) if e.kind() == io::ErrorKind::Interrupted => {
            PrompterError::Cancelled
        }
        dialoguer::Error::IO(e) => PrompterError::Io(e),
    }
}

impl Prompter for StdinPrompter {
    fn select(&self, prompt: &str, options: &[&str], default: usize) -> Result<usize> {
        use dialoguer::{theme::ColorfulTheme, Select};

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .items(options)
            .default(default)
            .interact_opt()
            .map_err(dialoguer_error_to_prompter)?;

        match selection {
            Some(idx) => Ok(idx),
            None => Err(PrompterError::Cancelled.into()),
        }
    }

    fn multi_select(
        &self,
        prompt: &str,
        options: &[&str],
        defaults: &[bool],
    ) -> Result<Vec<usize>> {
        use dialoguer::{theme::ColorfulTheme, MultiSelect};

        let selection = MultiSelect::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .items(options)
            .defaults(defaults)
            .interact_opt()
            .map_err(dialoguer_error_to_prompter)?;

        match selection {
            Some(indices) => Ok(indices),
            None => Err(PrompterError::Cancelled.into()),
        }
    }

    fn confirm(&self, prompt: &str, default: bool) -> Result<bool> {
        use dialoguer::{theme::ColorfulTheme, Confirm};

        let result = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .default(default)
            .interact_opt()
            .map_err(dialoguer_error_to_prompter)?;

        match result {
            Some(answer) => Ok(answer),
            None => Err(PrompterError::Cancelled.into()),
        }
    }

    fn info(&self, message: &str) {
        use console::style;
        println!("{} {}", style("ℹ").cyan().bold(), message);
    }

    fn show_diff(&self, path: &str, before: &str, after: &str) {
        use console::style;
        println!("{}", style(format!("--- {}", path)).red());
        println!("{}", style(format!("+++ {}", path)).green());
        for line in before.lines() {
            println!("{}", style(format!("- {}", line)).red());
        }
        for line in after.lines() {
            println!("{}", style(format!("+ {}", line)).green());
        }
    }
}

/// Represents a pre-configured mock response for a specific prompt type.
#[derive(Debug, Clone)]
pub enum MockResponse {
    /// Response for a `select()` call — the selected index.
    Select(usize),
    /// Response for a `multi_select()` call — the selected indices.
    MultiSelect(Vec<usize>),
    /// Response for a `confirm()` call — the yes/no answer.
    Confirm(bool),
    /// Simulate a cancellation (Ctrl+C / EOF).
    Cancel,
}

/// A record of a prompt that was shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptCall {
    Select {
        prompt: String,
        options: Vec<String>,
        default: usize,
    },
    MultiSelect {
        prompt: String,
        options: Vec<String>,
        defaults: Vec<bool>,
    },
    Confirm {
        prompt: String,
        default: bool,
    },
    Info {
        message: String,
    },
    ShowDiff {
        path: String,
        before: String,
        after: String,
    },
}

/// Test mock that replays pre-configured responses.
///
/// Responses are consumed in order. If responses are exhausted, subsequent
/// calls return an error. Call history is tracked for test assertions.
pub struct MockPrompter {
    responses: RefCell<Vec<MockResponse>>,
    call_history: RefCell<Vec<PromptCall>>,
}

impl MockPrompter {
    /// Create a new mock prompter with the given pre-configured responses.
    ///
    /// Responses are consumed in FIFO order as prompts are invoked.
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: RefCell::new(responses),
            call_history: RefCell::new(Vec::new()),
        }
    }

    /// Get the call history for test assertions.
    pub fn call_history(&self) -> Vec<PromptCall> {
        self.call_history.borrow().clone()
    }

    /// Get the number of remaining unconsumed responses.
    pub fn remaining_responses(&self) -> usize {
        self.responses.borrow().len()
    }

    /// Pop the next response, returning an error if exhausted or if Cancel.
    fn next_response(&self) -> Result<MockResponse> {
        let mut responses = self.responses.borrow_mut();
        if responses.is_empty() {
            anyhow::bail!("MockPrompter: no more responses configured");
        }
        let response = responses.remove(0);
        if matches!(response, MockResponse::Cancel) {
            return Err(PrompterError::Cancelled.into());
        }
        Ok(response)
    }
}

impl Prompter for MockPrompter {
    fn select(&self, prompt: &str, options: &[&str], default: usize) -> Result<usize> {
        self.call_history.borrow_mut().push(PromptCall::Select {
            prompt: prompt.to_string(),
            options: options.iter().map(|s| s.to_string()).collect(),
            default,
        });

        let response = self.next_response()?;
        match response {
            MockResponse::Select(idx) => Ok(idx),
            _ => anyhow::bail!("MockPrompter: expected Select response, got {:?}", response),
        }
    }

    fn multi_select(
        &self,
        prompt: &str,
        options: &[&str],
        defaults: &[bool],
    ) -> Result<Vec<usize>> {
        self.call_history
            .borrow_mut()
            .push(PromptCall::MultiSelect {
                prompt: prompt.to_string(),
                options: options.iter().map(|s| s.to_string()).collect(),
                defaults: defaults.to_vec(),
            });

        let response = self.next_response()?;
        match response {
            MockResponse::MultiSelect(indices) => Ok(indices),
            _ => anyhow::bail!(
                "MockPrompter: expected MultiSelect response, got {:?}",
                response
            ),
        }
    }

    fn confirm(&self, prompt: &str, default: bool) -> Result<bool> {
        self.call_history.borrow_mut().push(PromptCall::Confirm {
            prompt: prompt.to_string(),
            default,
        });

        let response = self.next_response()?;
        match response {
            MockResponse::Confirm(answer) => Ok(answer),
            _ => anyhow::bail!(
                "MockPrompter: expected Confirm response, got {:?}",
                response
            ),
        }
    }

    fn info(&self, message: &str) {
        self.call_history.borrow_mut().push(PromptCall::Info {
            message: message.to_string(),
        });
    }

    fn show_diff(&self, path: &str, before: &str, after: &str) {
        self.call_history.borrow_mut().push(PromptCall::ShowDiff {
            path: path.to_string(),
            before: before.to_string(),
            after: after.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_prompter_select() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(1)]);
        let result = prompter.select("Pick one:", &["a", "b", "c"], 0).unwrap();
        assert_eq!(result, 1);

        let history = prompter.call_history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            PromptCall::Select {
                prompt: "Pick one:".to_string(),
                options: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                default: 0,
            }
        );
    }

    #[test]
    fn test_mock_prompter_multi_select() {
        let prompter = MockPrompter::new(vec![MockResponse::MultiSelect(vec![0, 2])]);
        let result = prompter
            .multi_select("Pick many:", &["x", "y", "z"], &[true, false, true])
            .unwrap();
        assert_eq!(result, vec![0, 2]);

        let history = prompter.call_history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            PromptCall::MultiSelect {
                prompt: "Pick many:".to_string(),
                options: vec!["x".to_string(), "y".to_string(), "z".to_string()],
                defaults: vec![true, false, true],
            }
        );
    }

    #[test]
    fn test_mock_prompter_confirm() {
        let prompter = MockPrompter::new(vec![MockResponse::Confirm(true)]);
        let result = prompter.confirm("Are you sure?", false).unwrap();
        assert!(result);

        let history = prompter.call_history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            PromptCall::Confirm {
                prompt: "Are you sure?".to_string(),
                default: false,
            }
        );
    }

    #[test]
    fn test_mock_prompter_info_tracked() {
        let prompter = MockPrompter::new(vec![]);
        prompter.info("Hello, world!");

        let history = prompter.call_history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            PromptCall::Info {
                message: "Hello, world!".to_string(),
            }
        );
    }

    #[test]
    fn test_mock_prompter_show_diff_tracked() {
        let prompter = MockPrompter::new(vec![]);
        prompter.show_diff("config.json", "{}", "{\"key\": \"value\"}");

        let history = prompter.call_history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            PromptCall::ShowDiff {
                path: "config.json".to_string(),
                before: "{}".to_string(),
                after: "{\"key\": \"value\"}".to_string(),
            }
        );
    }

    #[test]
    fn test_mock_prompter_cancel() {
        let prompter = MockPrompter::new(vec![MockResponse::Cancel]);
        let result = prompter.select("Pick one:", &["a", "b"], 0);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(err.downcast_ref::<PrompterError>().is_some());
        let prompter_err = err.downcast_ref::<PrompterError>().unwrap();
        assert_eq!(prompter_err.exit_code(), 130);
    }

    #[test]
    fn test_mock_prompter_exhausted_responses() {
        let prompter = MockPrompter::new(vec![]);
        let result = prompter.select("Pick one:", &["a"], 0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no more responses configured"));
    }

    #[test]
    fn test_mock_prompter_wrong_response_type() {
        let prompter = MockPrompter::new(vec![MockResponse::Confirm(true)]);
        let result = prompter.select("Pick one:", &["a"], 0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected Select response"));
    }

    #[test]
    fn test_mock_prompter_sequential_responses() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Select(0),
            MockResponse::MultiSelect(vec![1]),
            MockResponse::Confirm(false),
        ]);

        let r1 = prompter.select("First:", &["a", "b"], 0).unwrap();
        assert_eq!(r1, 0);

        let r2 = prompter
            .multi_select("Second:", &["x", "y"], &[false, false])
            .unwrap();
        assert_eq!(r2, vec![1]);

        let r3 = prompter.confirm("Third:", true).unwrap();
        assert!(!r3);

        assert_eq!(prompter.remaining_responses(), 0);
        assert_eq!(prompter.call_history().len(), 3);
    }

    #[test]
    fn test_mock_prompter_cancel_records_history() {
        let prompter = MockPrompter::new(vec![MockResponse::Cancel]);
        let _ = prompter.confirm("Cancel me?", true);

        // The call should still be recorded in history even though it was cancelled
        let history = prompter.call_history();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            PromptCall::Confirm {
                prompt: "Cancel me?".to_string(),
                default: true,
            }
        );
    }

    #[test]
    fn test_prompter_error_display() {
        let cancelled = PrompterError::Cancelled;
        assert_eq!(cancelled.to_string(), "Operation cancelled by user");
        assert_eq!(cancelled.exit_code(), 130);

        let io_err = PrompterError::Io(io::Error::new(io::ErrorKind::BrokenPipe, "pipe broken"));
        assert!(io_err.to_string().contains("pipe broken"));
        assert_eq!(io_err.exit_code(), 1);
    }

    #[test]
    fn test_prompter_error_is_send_sync() {
        // Ensure PrompterError can be used across thread boundaries
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<PrompterError>();
        assert_sync::<PrompterError>();
    }
}
