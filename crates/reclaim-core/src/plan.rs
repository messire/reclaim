//! Run plan: which processes a run will act on and why the others are skipped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{ImageName, ProcessInfo, ProcessKey};
use crate::policy::{Context, Eligibility, LISTS_VERSION, Policy, evaluate};
use crate::units::Bytes;

/// What the user asked to include.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Selection {
    /// Every eligible process.
    #[default]
    AllEligible,
    /// Only these processes, if they are eligible.
    Only(Vec<ProcessKey>),
}

/// A process the run will act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Identity, validated again at open time.
    pub key: ProcessKey,
    /// Image name.
    pub image: ImageName,
    /// Working set when the plan was made (ordering key).
    pub working_set: Bytes,
}

/// A process the run will not act on, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skip {
    /// Identity.
    pub key: ProcessKey,
    /// Image name.
    pub image: ImageName,
    /// Working set when known.
    pub working_set: Option<Bytes>,
    /// Why it is skipped.
    pub reason: Eligibility,
}

/// The plan shown to the user before a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Identifier echoed by `StartRun`.
    pub id: u64,
    /// Version of the built-in protection lists used.
    pub lists_version: u32,
    /// Targets, largest working set first.
    pub targets: Vec<Target>,
    /// Skipped processes, in enumeration order.
    pub skipped: Vec<Skip>,
}

impl Plan {
    /// Builds a plan from an enumeration.
    pub fn build(
        id: u64,
        processes: &[ProcessInfo],
        policy: &Policy,
        ctx: &Context,
        selection: &Selection,
    ) -> Plan {
        let mut targets = Vec::new();
        let mut skipped = Vec::new();
        for p in processes {
            let selected = match selection {
                Selection::AllEligible => true,
                Selection::Only(keys) => keys.contains(&p.key),
            };
            if !selected {
                continue;
            }
            let reason = evaluate(p, policy, ctx);
            let ws = p.memory.map(|m| m.working_set);
            match (reason, ws) {
                (Eligibility::Eligible, Some(working_set)) => targets.push(Target {
                    key: p.key,
                    image: p.image.clone(),
                    working_set,
                }),
                (reason, _) => skipped.push(Skip {
                    key: p.key,
                    image: p.image.clone(),
                    working_set: ws,
                    reason,
                }),
            }
        }
        targets.sort_by(|a, b| {
            b.working_set
                .cmp(&a.working_set)
                .then(a.key.pid.cmp(&b.key.pid))
        });
        Plan {
            id,
            lists_version: LISTS_VERSION,
            targets,
            skipped,
        }
    }

    /// Sum of target working sets at planning time (an upper bound, not a promise).
    pub fn targeted_working_set(&self) -> Bytes {
        self.targets.iter().map(|t| t.working_set).sum()
    }

    /// Number of skipped processes per reason key, sorted by key.
    pub fn skip_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut m = BTreeMap::new();
        for s in &self.skipped {
            *m.entry(s.reason.key()).or_insert(0) += 1;
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::tests::proc_;

    fn ctx() -> Context {
        Context {
            self_pid: 1,
            foreground: None,
        }
    }

    #[test]
    fn orders_largest_first_and_counts_skips() {
        let procs = vec![
            proc_(10, "a.exe", 100),
            proc_(11, "b.exe", 400),
            proc_(12, "c.exe", 400),
            proc_(13, "tiny.exe", 1),
            proc_(4, "System", 500),
            proc_(1, "reclaim.exe", 50),
        ];
        let plan = Plan::build(
            7,
            &procs,
            &Policy::default(),
            &ctx(),
            &Selection::AllEligible,
        );
        let order: Vec<u32> = plan.targets.iter().map(|t| t.key.pid).collect();
        assert_eq!(order, vec![11, 12, 10]);
        assert_eq!(plan.targeted_working_set(), Bytes::from_mib(900));
        let counts = plan.skip_counts();
        assert_eq!(counts.get("reason.below_threshold"), Some(&1));
        assert_eq!(counts.get("reason.system_critical"), Some(&1));
        assert_eq!(counts.get("reason.self"), Some(&1));
        assert_eq!(plan.id, 7);
        assert_eq!(plan.lists_version, LISTS_VERSION);
    }

    #[test]
    fn selection_only_limits_and_still_applies_policy() {
        let procs = vec![
            proc_(10, "a.exe", 100),
            proc_(11, "b.exe", 400),
            proc_(4, "System", 500),
        ];
        let sel = Selection::Only(vec![procs[0].key, procs[2].key]);
        let plan = Plan::build(1, &procs, &Policy::default(), &ctx(), &sel);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].key.pid, 10);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, Eligibility::SystemCritical);
    }

    #[test]
    fn empty_plan() {
        let plan = Plan::build(1, &[], &Policy::default(), &ctx(), &Selection::default());
        assert!(plan.targets.is_empty());
        assert_eq!(plan.targeted_working_set(), Bytes::ZERO);
        assert!(plan.skip_counts().is_empty());
    }
}
