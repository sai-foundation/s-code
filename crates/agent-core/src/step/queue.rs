use super::{AdmissionTarget, StepAdmissionError, StepRequest};
use opencoding_model_gateway::ModelMessage;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueuePosition {
    Head,
    #[default]
    Tail,
}

#[derive(Clone, Debug)]
pub struct StepRequestBatch {
    pub driver: StepRequest,
    pub merged: Vec<StepRequest>,
}

impl StepRequestBatch {
    pub fn materialize(mut self) -> Vec<ModelMessage> {
        let mut messages = self.driver.materialize();
        for request in &mut self.merged {
            messages.extend(request.materialize());
        }
        messages
    }
}

#[derive(Clone, Debug, Default)]
pub struct StepRequestQueue {
    items: VecDeque<StepRequest>,
}

impl StepRequestQueue {
    pub fn admit(
        &mut self,
        request: StepRequest,
        has_active_turn: bool,
        position: QueuePosition,
    ) -> Result<AdmissionTarget, StepAdmissionError> {
        let target = request.admission.target(has_active_turn)?;
        self.enqueue(request, position);
        Ok(target)
    }

    pub fn enqueue(&mut self, request: StepRequest, position: QueuePosition) {
        match position {
            QueuePosition::Head => self.items.push_front(request),
            QueuePosition::Tail => self.items.push_back(request),
        }
    }

    pub fn has_pending_requests(&self) -> bool {
        self.items.iter().any(|request| !request.is_aborted())
    }

    /// Selects one non-mergeable driver and folds every currently mergeable
    /// request into the same model Step. Other drivers retain FIFO order.
    pub fn take_next_batch(&mut self) -> Option<StepRequestBatch> {
        self.discard_aborted();
        if self.items.is_empty() {
            return None;
        }
        let driver_index = self
            .items
            .iter()
            .position(|request| !request.mergeable)
            .unwrap_or(0);
        let driver = self.items.remove(driver_index)?;
        let mut merged = Vec::new();
        let mut rest = VecDeque::new();
        while let Some(request) = self.items.pop_front() {
            if request.mergeable {
                merged.push(request);
            } else {
                rest.push_back(request);
            }
        }
        self.items = rest;
        Some(StepRequestBatch { driver, merged })
    }

    pub fn abort_turn_scoped(&mut self) {
        for request in &mut self.items {
            if request.turn_scoped {
                request.abort();
            }
        }
        self.discard_aborted();
    }

    pub fn drain(&mut self) -> Vec<StepRequest> {
        self.items.drain(..).collect()
    }

    pub fn drain_materialized_messages(&mut self) -> Vec<ModelMessage> {
        let mut messages = Vec::new();
        while let Some(batch) = self.take_next_batch() {
            messages.extend(batch.materialize());
        }
        messages
    }

    fn discard_aborted(&mut self) {
        self.items.retain(|request| !request.is_aborted());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step::{StepAdmission, StepRequestOptions};
    use serde_json::Value;

    fn request(label: &str, mergeable: bool, turn_scoped: bool) -> StepRequest {
        StepRequest::new(
            label,
            vec![ModelMessage {
                role: "user".into(),
                content: Value::String(label.into()),
            }],
            StepRequestOptions {
                admission: StepAdmission::ActiveOrNextTurn,
                mergeable,
                turn_scoped,
            },
        )
    }

    fn text(messages: &[ModelMessage]) -> Vec<&str> {
        messages
            .iter()
            .filter_map(|message| message.content.as_str())
            .collect()
    }

    #[test]
    fn one_driver_absorbs_all_mergeable_requests() {
        let mut queue = StepRequestQueue::default();
        queue.enqueue(request("steer-before", true, false), QueuePosition::Tail);
        queue.enqueue(request("driver-one", false, true), QueuePosition::Tail);
        queue.enqueue(request("tool-result", true, true), QueuePosition::Tail);
        queue.enqueue(request("driver-two", false, true), QueuePosition::Tail);

        let first = queue.take_next_batch().unwrap();
        assert_eq!(first.driver.kind, "driver-one");
        assert_eq!(
            first
                .merged
                .iter()
                .map(|request| request.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["steer-before", "tool-result"]
        );
        assert_eq!(queue.take_next_batch().unwrap().driver.kind, "driver-two");
        assert!(!queue.has_pending_requests());
    }

    #[test]
    fn two_drivers_produce_two_steps() {
        let mut queue = StepRequestQueue::default();
        queue.enqueue(request("first", false, true), QueuePosition::Tail);
        queue.enqueue(request("second", false, true), QueuePosition::Tail);

        assert_eq!(queue.take_next_batch().unwrap().driver.kind, "first");
        assert_eq!(queue.take_next_batch().unwrap().driver.kind, "second");
    }

    #[test]
    fn head_insertion_makes_a_retry_run_first() {
        let mut queue = StepRequestQueue::default();
        queue.enqueue(request("later", false, true), QueuePosition::Tail);
        let mut retry = request("retry", false, true);
        assert_eq!(text(&retry.materialize()), vec!["retry"]);
        queue.enqueue(retry, QueuePosition::Head);

        let retry_batch = queue.take_next_batch().unwrap();
        assert_eq!(retry_batch.driver.kind, "retry");
        assert_eq!(text(&retry_batch.materialize()), vec!["retry"]);
    }

    #[test]
    fn turn_cancellation_preserves_agent_scoped_requests() {
        let mut queue = StepRequestQueue::default();
        queue.enqueue(request("turn", false, true), QueuePosition::Tail);
        queue.enqueue(request("agent-steer", true, false), QueuePosition::Tail);

        queue.abort_turn_scoped();

        let messages = queue.drain_materialized_messages();
        assert_eq!(text(&messages), vec!["agent-steer"]);
    }

    #[test]
    fn admission_is_checked_before_a_request_enters_the_queue() {
        let mut queue = StepRequestQueue::default();
        let active_only = StepRequest::new(
            "active-only",
            Vec::new(),
            StepRequestOptions {
                admission: StepAdmission::ActiveTurnOnly,
                mergeable: false,
                turn_scoped: true,
            },
        );

        assert_eq!(
            queue.admit(active_only, false, QueuePosition::Tail),
            Err(crate::step::StepAdmissionError::ActiveTurnRequired)
        );
        assert!(!queue.has_pending_requests());
    }
}
