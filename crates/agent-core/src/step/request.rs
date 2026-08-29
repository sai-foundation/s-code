use opencoding_model_gateway::ModelMessage;
use opencoding_protocol::Id;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Determines which Turn owns a newly admitted Step request.
///
/// Admission is deliberately independent from message materialization. A caller
/// can reject or cancel a request before any of its messages enter model context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepAdmission {
    NewTurn,
    ActiveOrNewTurn,
    ActiveOrNextTurn,
    ActiveTurnOnly,
}

impl StepAdmission {
    pub fn target(self, has_active_turn: bool) -> Result<AdmissionTarget, StepAdmissionError> {
        match (self, has_active_turn) {
            (Self::NewTurn, _) | (Self::ActiveOrNewTurn, false) => Ok(AdmissionTarget::NewTurn),
            (Self::ActiveOrNextTurn, false) => Ok(AdmissionTarget::NextTurn),
            (Self::ActiveTurnOnly, false) => Err(StepAdmissionError::ActiveTurnRequired),
            (Self::ActiveOrNewTurn | Self::ActiveOrNextTurn | Self::ActiveTurnOnly, true) => {
                Ok(AdmissionTarget::ActiveTurn)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionTarget {
    ActiveTurn,
    NewTurn,
    NextTurn,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum StepAdmissionError {
    #[error("step request requires an active turn")]
    ActiveTurnRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRequestState {
    Pending,
    Materialized,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepRequestOptions {
    pub admission: StepAdmission,
    pub mergeable: bool,
    pub turn_scoped: bool,
}

impl Default for StepRequestOptions {
    fn default() -> Self {
        Self {
            admission: StepAdmission::ActiveOrNextTurn,
            mergeable: false,
            turn_scoped: true,
        }
    }
}

/// One lazily materialized unit of work for an Agent model Step.
///
/// The messages are plain data, but they are not added to model context until
/// the request is selected in a batch. This keeps cancellation compensation-free
/// and makes retries safe to put back at the head of a queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRequest {
    pub id: Id,
    pub kind: String,
    pub admission: StepAdmission,
    pub mergeable: bool,
    pub turn_scoped: bool,
    state: StepRequestState,
    messages: Vec<ModelMessage>,
}

impl StepRequest {
    pub fn new(
        kind: impl Into<String>,
        messages: Vec<ModelMessage>,
        options: StepRequestOptions,
    ) -> Self {
        Self {
            id: Id::new("step"),
            kind: kind.into(),
            admission: options.admission,
            mergeable: options.mergeable,
            turn_scoped: options.turn_scoped,
            state: StepRequestState::Pending,
            messages,
        }
    }

    pub fn continuation(kind: impl Into<String>) -> Self {
        Self::new(
            kind,
            Vec::new(),
            StepRequestOptions {
                admission: StepAdmission::ActiveTurnOnly,
                mergeable: false,
                turn_scoped: true,
            },
        )
    }

    pub fn state(&self) -> StepRequestState {
        self.state
    }

    pub fn is_aborted(&self) -> bool {
        self.state == StepRequestState::Aborted
    }

    pub fn abort(&mut self) -> bool {
        if self.state != StepRequestState::Pending {
            return false;
        }
        self.state = StepRequestState::Aborted;
        true
    }

    pub fn materialize(&mut self) -> Vec<ModelMessage> {
        match self.state {
            StepRequestState::Pending => self.state = StepRequestState::Materialized,
            StepRequestState::Materialized => {}
            StepRequestState::Aborted => return Vec::new(),
        }
        self.messages.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn message(text: &str) -> ModelMessage {
        ModelMessage {
            role: "user".into(),
            content: Value::String(text.into()),
        }
    }

    #[test]
    fn all_admission_modes_are_explicit() {
        assert_eq!(
            StepAdmission::NewTurn.target(true),
            Ok(AdmissionTarget::NewTurn)
        );
        assert_eq!(
            StepAdmission::ActiveOrNewTurn.target(false),
            Ok(AdmissionTarget::NewTurn)
        );
        assert_eq!(
            StepAdmission::ActiveOrNewTurn.target(true),
            Ok(AdmissionTarget::ActiveTurn)
        );
        assert_eq!(
            StepAdmission::ActiveOrNextTurn.target(false),
            Ok(AdmissionTarget::NextTurn)
        );
        assert_eq!(
            StepAdmission::ActiveOrNextTurn.target(true),
            Ok(AdmissionTarget::ActiveTurn)
        );
        assert_eq!(
            StepAdmission::ActiveTurnOnly.target(false),
            Err(StepAdmissionError::ActiveTurnRequired)
        );
        assert_eq!(
            StepAdmission::ActiveTurnOnly.target(true),
            Ok(AdmissionTarget::ActiveTurn)
        );
    }

    #[test]
    fn aborted_request_never_materializes_context() {
        let mut request =
            StepRequest::new("steer", vec![message("do not append")], Default::default());
        assert!(request.abort());
        assert!(request.materialize().is_empty());
        assert_eq!(request.state(), StepRequestState::Aborted);
    }

    #[test]
    fn materialization_is_repeatable_for_a_retried_driver() {
        let mut request =
            StepRequest::new("retry", vec![message("same context")], Default::default());
        let first = request.materialize();
        let second = request.materialize();

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].content, Value::String("same context".into()));
        assert_eq!(request.state(), StepRequestState::Materialized);
    }
}
