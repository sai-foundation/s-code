use super::ResourceClaim;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct ScheduledTool<T> {
    pub value: T,
    pub claims: Vec<ResourceClaim>,
}

impl<T> ScheduledTool<T> {
    pub fn new(value: T, claims: Vec<ResourceClaim>) -> Self {
        Self {
            value,
            claims: normalized_claims(claims),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolSchedule<T> {
    waiting: VecDeque<ScheduledTool<T>>,
}

impl<T> Default for ToolSchedule<T> {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
        }
    }
}

impl<T> ToolSchedule<T> {
    pub fn push(&mut self, value: T, claims: Vec<ResourceClaim>) {
        self.waiting.push_back(ScheduledTool::new(value, claims));
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    pub fn drain(&mut self) -> impl Iterator<Item = ScheduledTool<T>> + '_ {
        self.waiting.drain(..)
    }

    /// Returns the largest safe FIFO wave. A request cannot pass an earlier
    /// waiting request when their claims conflict, which prevents a stream of
    /// readers from starving an earlier writer.
    pub fn take_ready_wave(&mut self) -> Vec<ScheduledTool<T>> {
        let mut ready = Vec::new();
        let mut blocked = VecDeque::new();
        while let Some(candidate) = self.waiting.pop_front() {
            let conflicts_with_running = ready
                .iter()
                .any(|running| tools_conflict(&candidate, running));
            let passes_conflicting_waiter = blocked
                .iter()
                .any(|waiting| tools_conflict(&candidate, waiting));
            if conflicts_with_running || passes_conflicting_waiter {
                blocked.push_back(candidate);
            } else {
                ready.push(candidate);
            }
        }
        self.waiting = blocked;
        ready
    }
}

fn normalized_claims(claims: Vec<ResourceClaim>) -> Vec<ResourceClaim> {
    if claims.is_empty() {
        vec![ResourceClaim::global_exclusive()]
    } else {
        claims
    }
}

fn tools_conflict<T>(left: &ScheduledTool<T>, right: &ScheduledTool<T>) -> bool {
    left.claims.iter().any(|left_claim| {
        right
            .claims
            .iter()
            .any(|right_claim| left_claim.conflicts_with(right_claim))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ResourceMode;

    fn claim(path: &str, mode: ResourceMode) -> Vec<ResourceClaim> {
        vec![ResourceClaim::workspace(path, mode, false).unwrap()]
    }

    #[test]
    fn independent_reads_and_writes_share_a_wave() {
        let mut schedule = ToolSchedule::default();
        schedule.push("read-a", claim("a", ResourceMode::Read));
        schedule.push("read-a-again", claim("a", ResourceMode::Search));
        schedule.push("write-b", claim("b", ResourceMode::Write));

        let wave = schedule.take_ready_wave();
        assert_eq!(
            wave.into_iter().map(|tool| tool.value).collect::<Vec<_>>(),
            vec!["read-a", "read-a-again", "write-b"]
        );
        assert!(schedule.is_empty());
    }

    #[test]
    fn conflicting_calls_keep_fifo_order() {
        let mut schedule = ToolSchedule::default();
        schedule.push("write", claim("a", ResourceMode::Write));
        schedule.push("read", claim("a", ResourceMode::Read));
        schedule.push("write-again", claim("a", ResourceMode::Write));

        assert_eq!(schedule.take_ready_wave()[0].value, "write");
        assert_eq!(schedule.take_ready_wave()[0].value, "read");
        assert_eq!(schedule.take_ready_wave()[0].value, "write-again");
    }

    #[test]
    fn an_earlier_writer_is_not_starved_by_later_readers() {
        let mut schedule = ToolSchedule::default();
        schedule.push("first-reader", claim("a", ResourceMode::Read));
        schedule.push("writer", claim("a", ResourceMode::Write));
        schedule.push("later-reader", claim("a", ResourceMode::Read));

        let first = schedule.take_ready_wave();
        assert_eq!(first[0].value, "first-reader");
        let second = schedule.take_ready_wave();
        assert_eq!(second[0].value, "writer");
        let third = schedule.take_ready_wave();
        assert_eq!(third[0].value, "later-reader");
    }

    #[test]
    fn missing_claims_fail_closed_to_global_exclusive() {
        let mut schedule = ToolSchedule::default();
        schedule.push("unknown", Vec::new());
        schedule.push("read", claim("a", ResourceMode::Read));

        assert_eq!(schedule.take_ready_wave()[0].value, "unknown");
        assert_eq!(schedule.take_ready_wave()[0].value, "read");
    }
}
