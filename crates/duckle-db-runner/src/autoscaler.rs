//! Deterministic elastic warm-pool policy.

use crate::demand::{DemandObservation, DEMAND_WINDOW_MILLIS};

pub const AUTOSCALE_INTERVAL_MILLIS: u64 = 5_000;
pub const HEADROOM_PERCENT: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleAction {
    Hold,
    ScaleOut { workers: u32 },
    ScaleIn { workers: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoscaleDecision {
    pub active_demand: u32,
    pub peak_5m: u32,
    pub base_capacity: u32,
    pub current_warm_capacity: u32,
    pub target_warm_capacity: u32,
    pub action: ScaleAction,
}

/// Holds only controller state. The peak itself is deliberately reset on
/// restart with the `DemandWindow`, so no historical high-water target leaks
/// into a new workspace instance.
#[derive(Debug, Clone)]
pub struct ElasticAutoscaler {
    base_capacity: u32,
    last_evaluation_millis: Option<u64>,
}

impl ElasticAutoscaler {
    pub fn new(base_capacity: u32) -> Self {
        assert!(base_capacity > 0, "base capacity must be positive");
        Self {
            base_capacity,
            last_evaluation_millis: None,
        }
    }

    pub fn base_capacity(&self) -> u32 {
        self.base_capacity
    }

    pub fn set_base_capacity(&mut self, base_capacity: u32) {
        assert!(base_capacity > 0, "base capacity must be positive");
        self.base_capacity = base_capacity;
    }

    pub fn target_for_peak(&self, peak_5m: u32) -> u32 {
        // ceil(peak * 1.20), implemented without floating point rounding.
        let with_headroom = peak_5m
            .saturating_mul(100 + HEADROOM_PERCENT)
            .saturating_add(99)
            / 100;
        self.base_capacity.max(with_headroom)
    }

    pub fn evaluate_if_due(
        &mut self,
        now_millis: u64,
        demand: DemandObservation,
        current_warm_capacity: u32,
    ) -> Option<AutoscaleDecision> {
        if self
            .last_evaluation_millis
            .is_some_and(|last| now_millis.saturating_sub(last) < AUTOSCALE_INTERVAL_MILLIS)
        {
            return None;
        }
        Some(self.evaluate_now(now_millis, demand, current_warm_capacity))
    }

    /// Used for startup and a base-capacity save. Demand still comes from the
    /// same five-minute ledger; the save does not introduce a second resize
    /// mechanism.
    pub fn evaluate_now(
        &mut self,
        now_millis: u64,
        demand: DemandObservation,
        current_warm_capacity: u32,
    ) -> AutoscaleDecision {
        self.last_evaluation_millis = Some(now_millis);
        let target_warm_capacity = self.target_for_peak(demand.peak_5m);
        let action = if target_warm_capacity > current_warm_capacity {
            ScaleAction::ScaleOut {
                workers: target_warm_capacity - current_warm_capacity,
            }
        } else if target_warm_capacity < current_warm_capacity {
            ScaleAction::ScaleIn {
                workers: current_warm_capacity - target_warm_capacity,
            }
        } else {
            ScaleAction::Hold
        };
        AutoscaleDecision {
            active_demand: demand.active_runs,
            peak_5m: demand.peak_5m,
            base_capacity: self.base_capacity,
            current_warm_capacity,
            target_warm_capacity,
            action,
        }
    }

    pub const fn demand_window_millis() -> u64 {
        DEMAND_WINDOW_MILLIS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demand(peak_5m: u32) -> DemandObservation {
        DemandObservation {
            active_runs: 0,
            peak_5m,
            newly_counted: false,
        }
    }

    #[test]
    fn uses_base_three_and_integer_ceiling() {
        let scaler = ElasticAutoscaler::new(3);
        assert_eq!(scaler.target_for_peak(0), 3);
        assert_eq!(scaler.target_for_peak(1), 3);
        assert_eq!(scaler.target_for_peak(100), 120);
        assert_eq!(scaler.target_for_peak(101), 122);
    }

    #[test]
    fn evaluates_every_five_seconds_and_scales_in_only_after_peak_changes() {
        let mut scaler = ElasticAutoscaler::new(3);
        let first = scaler.evaluate_if_due(0, demand(100), 3).unwrap();
        assert_eq!(first.action, ScaleAction::ScaleOut { workers: 117 });
        assert!(scaler.evaluate_if_due(4_999, demand(100), 120).is_none());
        let hold = scaler.evaluate_if_due(5_000, demand(100), 120).unwrap();
        assert_eq!(hold.action, ScaleAction::Hold);
        let scale_in = scaler.evaluate_if_due(10_000, demand(0), 120).unwrap();
        assert_eq!(scale_in.action, ScaleAction::ScaleIn { workers: 117 });
    }
}
