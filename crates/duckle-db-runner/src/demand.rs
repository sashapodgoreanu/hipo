//! Event-time concurrent demand ledger used by the elastic warm pool.

use crate::model::RunId;
use std::collections::{HashSet, VecDeque};

pub const DEMAND_WINDOW_MILLIS: u64 = 5 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DemandSample {
    at_millis: u64,
    concurrent_runs: u32,
}

/// Records every acquire and terminal event. The controller, not a provider,
/// owns the ledger, so pool-backed and on-demand runs count identically.
#[derive(Debug, Default)]
pub struct DemandWindow {
    active_runs: HashSet<RunId>,
    samples: VecDeque<DemandSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemandObservation {
    pub active_runs: u32,
    pub peak_5m: u32,
    pub newly_counted: bool,
}

impl DemandWindow {
    pub fn acquire(&mut self, run_id: RunId, now_millis: u64) -> DemandObservation {
        self.expire(now_millis);
        let newly_counted = self.active_runs.insert(run_id);
        if newly_counted {
            self.samples.push_back(DemandSample {
                at_millis: now_millis,
                concurrent_runs: self.active_runs.len() as u32,
            });
        }
        self.observe(now_millis, newly_counted)
    }

    pub fn terminal(&mut self, run_id: RunId, now_millis: u64) -> DemandObservation {
        self.expire(now_millis);
        let changed = self.active_runs.remove(&run_id);
        if changed {
            self.samples.push_back(DemandSample {
                at_millis: now_millis,
                concurrent_runs: self.active_runs.len() as u32,
            });
        }
        self.observe(now_millis, false)
    }

    pub fn observe(&mut self, now_millis: u64, newly_counted: bool) -> DemandObservation {
        self.expire(now_millis);
        let active_runs = self.active_runs.len() as u32;
        let peak_5m = self
            .samples
            .iter()
            .map(|sample| sample.concurrent_runs)
            .max()
            .unwrap_or(0)
            .max(active_runs);
        DemandObservation {
            active_runs,
            peak_5m,
            newly_counted,
        }
    }

    fn expire(&mut self, now_millis: u64) {
        let earliest = now_millis.saturating_sub(DEMAND_WINDOW_MILLIS);
        while self
            .samples
            .front()
            .is_some_and(|sample| sample.at_millis < earliest)
        {
            self.samples.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_a_burst_once_and_expires_it_after_five_minutes() {
        let mut window = DemandWindow::default();
        let runs: Vec<_> = (0..100).map(|_| RunId::new()).collect();
        for run in &runs {
            window.acquire(*run, 0);
            window.acquire(*run, 0);
        }
        assert_eq!(window.observe(0, false).peak_5m, 100);
        for run in &runs {
            window.terminal(*run, 1_000);
        }
        assert_eq!(window.observe(DEMAND_WINDOW_MILLIS, false).peak_5m, 100);
        // The terminal samples were observed at t=1000ms, so their active
        // counts remain in the sliding window for five further minutes.
        assert_eq!(
            window.observe(DEMAND_WINDOW_MILLIS + 1_001, false).peak_5m,
            0
        );
    }
}
