//! The context-cap section: the main threads priced as if each session restarted through a
//! pickup before a call would send more than a cap, beside the pickup turns that price a
//! restart and set the context it leaves.

use super::price::{Cost, cost};
use super::range::Thread;
use super::sessions::spread;
use super::tally::{add, count_one, plus};
use super::transcript::{Call, Transcript, Usage};

/// The context-cap section. Every amount is in nanodollars; a call the price table lacks
/// costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapSection {
    /// The pickup turns opening a main session in the range, where any did.
    pub pickup: Option<Pickup>,
    pub simulation: Simulation,
}

/// The pickup turns a restart is priced at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pickup {
    pub turns: u64,
    /// What one turn's calls cost, at the median.
    pub median_nanodollars: u64,
    /// The context one turn's last call sent, in tokens, at the median.
    pub median_context: u64,
}

/// What the simulation could price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Simulation {
    /// No cap was given.
    NoCap,
    /// No pickup turn opened a main session, so a restart has no price.
    NoPickupTurn {
        cap: u64,
    },
    /// The cap is at or below the context a pickup turn ends at, so a restart drops nothing.
    CapAtOrBelowPickup {
        cap: u64,
    },
    Ran(Outcome),
}

/// The main threads under the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub cap: u64,
    /// Main sessions holding a call in the range.
    pub sessions: u64,
    pub sessions_restarted: u64,
    pub restarts: u64,
    /// The main threads' input side as run.
    pub as_run: u128,
    /// The main threads' input side with each call's context less its dropped amount.
    pub simulated: u128,
    /// What the restarts' pickup turns cost.
    pub restart_cost: u128,
}

impl Outcome {
    /// How the simulated input side and the restarts together compare with the input side
    /// as run.
    #[must_use]
    pub fn difference(&self) -> Difference {
        let total = add(self.simulated, self.restart_cost);
        if total <= self.as_run {
            let Some(saved) = self.as_run.checked_sub(total) else {
                unreachable!("a total at most the cost as run leaves a saving")
            };
            Difference::Saved(saved)
        } else {
            let Some(added) = total.checked_sub(self.as_run) else {
                unreachable!("a total above the cost as run leaves an addition")
            };
            Difference::Added(added)
        }
    }
}

/// What a cap saves or adds, in nanodollars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difference {
    Saved(u128),
    Added(u128),
}

/// The context-cap section of the main threads in `transcripts` under `cap`.
#[must_use]
pub fn cap_section(transcripts: &[Transcript], cap: Option<u64>) -> CapSection {
    let pickup = pickup(transcripts);
    let simulation = match (cap, pickup) {
        (None, _) => Simulation::NoCap,
        (Some(cap), None) => Simulation::NoPickupTurn { cap },
        (Some(cap), Some(pickup)) => {
            let limits = Limits {
                cap,
                floor: pickup.median_context,
            };
            if cap > limits.floor {
                Simulation::Ran(outcome(transcripts, limits, pickup.median_nanodollars))
            } else {
                Simulation::CapAtOrBelowPickup { cap }
            }
        }
    };
    CapSection { pickup, simulation }
}

fn main_threads(transcripts: &[Transcript]) -> impl Iterator<Item = &Transcript> {
    transcripts
        .iter()
        .filter(|transcript| transcript.thread == Thread::Main)
}

/// The pickup turns opening a main session that began in the range, where any holds a call.
fn pickup(transcripts: &[Transcript]) -> Option<Pickup> {
    let mut costs = Vec::new();
    let mut contexts = Vec::new();
    for transcript in main_threads(transcripts) {
        if transcript.before_range.is_some() {
            continue;
        }
        let Some(turn) = transcript.pickup_turn else {
            continue;
        };
        let calls: Vec<&Call> = transcript
            .calls
            .iter()
            .filter(|call| turn.holds(call.timestamp))
            .collect();
        let Some(last) = calls.last() else {
            continue;
        };
        let mut nanodollars: u64 = 0;
        for call in &calls {
            let priced = cost(&call.model, &call.usage).unwrap_or_default();
            nanodollars = plus(nanodollars, priced.total());
        }
        costs.push(nanodollars);
        contexts.push(last.usage.context());
    }
    assert_eq!(
        costs.len(),
        contexts.len(),
        "each pickup turn has a cost and a context"
    );
    let median_nanodollars = spread(&costs)?.median;
    let Some(context) = spread(&contexts) else {
        unreachable!("pickup turns holding a cost hold a context")
    };
    let Ok(turns) = u64::try_from(costs.len()) else {
        unreachable!("a count of turns fits a u64")
    };
    Some(Pickup {
        turns,
        median_nanodollars,
        median_context: context.median,
    })
}

/// The cap a session restarts under, and the context a restart leaves, in tokens.
#[derive(Debug, Clone, Copy)]
struct Limits {
    cap: u64,
    floor: u64,
}

/// The main threads in `transcripts` under `limits`, each restart costing `restart`.
fn outcome(transcripts: &[Transcript], limits: Limits, restart: u64) -> Outcome {
    assert!(
        limits.cap > limits.floor,
        "a restart leaves less context than the cap"
    );
    let mut outcome = Outcome {
        cap: limits.cap,
        sessions: 0,
        sessions_restarted: 0,
        restarts: 0,
        as_run: 0,
        simulated: 0,
        restart_cost: 0,
    };
    for transcript in main_threads(transcripts) {
        if transcript.calls.is_empty() {
            continue;
        }
        let run = simulate(transcript, limits);
        outcome.sessions = count_one(outcome.sessions);
        if run.restarts > 0 {
            outcome.sessions_restarted = count_one(outcome.sessions_restarted);
        }
        outcome.restarts = plus(outcome.restarts, run.restarts);
        outcome.as_run = add(outcome.as_run, run.as_run);
        outcome.simulated = add(outcome.simulated, run.simulated);
    }
    let Some(restart_cost) = u128::from(restart).checked_mul(u128::from(outcome.restarts)) else {
        unreachable!("a bounded count of restarts at a bounded cost fits a u128")
    };
    outcome.restart_cost = restart_cost;
    assert!(
        outcome.sessions_restarted <= outcome.sessions,
        "the sessions restarted are among the sessions"
    );
    outcome
}

/// One session under the cap.
#[derive(Debug, Clone, Copy)]
struct Priced {
    restarts: u64,
    as_run: u128,
    simulated: u128,
}

/// One main session's calls in order under `limits`. A compaction boundary before a call
/// replaced the context a drop came from and the one a restart would start from, so it
/// sets the dropped amount back to zero and leaves no call before to restart from.
fn simulate(transcript: &Transcript, limits: Limits) -> Priced {
    let mut run = Priced {
        restarts: 0,
        as_run: 0,
        simulated: 0,
    };
    let mut dropped: u64 = 0;
    let mut previous = None;
    let mut compactions = transcript.compactions.iter().peekable();
    for call in &transcript.calls {
        // Each pass takes one boundary, so the loop ends within the boundaries' count.
        while compactions.next_if(|&&at| at <= call.timestamp).is_some() {
            dropped = 0;
            previous = None;
        }
        let step = Step {
            context: call.usage.context(),
            dropped,
            previous,
            limits,
        };
        if let Some(restarted) = restart(step) {
            dropped = restarted;
            run.restarts = count_one(run.restarts);
        }
        let taken = Step { dropped, ..step }.taken();
        let usage = reduced(&call.usage, taken);
        let as_run = cost(&call.model, &call.usage).unwrap_or_default();
        let simulated = cost(&call.model, &usage).unwrap_or_default();
        run.as_run = add(run.as_run, input_side(&as_run));
        run.simulated = add(run.simulated, input_side(&simulated));
        previous = Some(step.context);
    }
    assert!(
        run.simulated <= run.as_run,
        "a drop never raises a session's input side"
    );
    run
}

/// What the simulation knows at one call: the context it sent, the dropped amount standing,
/// and the context the call before sent, as run.
#[derive(Debug, Clone, Copy)]
struct Step {
    context: u64,
    dropped: u64,
    previous: Option<u64>,
    limits: Limits,
}

impl Step {
    /// The tokens the drop takes out of the call: the dropped amount, but no more than
    /// leaves the call the context a restart leaves.
    fn taken(self) -> u64 {
        let above_floor = self.context.saturating_sub(self.limits.floor);
        self.dropped.min(above_floor)
    }
}

/// The dropped amount a restart before the call leaves, where the call would send more
/// than the cap under the drop standing and a restart drops more: the context of the call
/// before, less the context a restart leaves.
fn restart(step: Step) -> Option<u64> {
    let Some(sent) = step.context.checked_sub(step.taken()) else {
        unreachable!("a drop takes at most the call's context")
    };
    if sent <= step.limits.cap {
        return None;
    }
    let before = step.previous?;
    let restarted = before.saturating_sub(step.limits.floor);
    if restarted > step.dropped {
        Some(restarted)
    } else {
        None
    }
}

/// `usage` less `taken` tokens, from its cache read, then its 1-hour cache write, then its
/// 5-minute cache write, then its uncached input.
fn reduced(usage: &Usage, taken: u64) -> Usage {
    let context = usage.context();
    assert!(
        taken <= context,
        "a drop takes at most the call's context: {taken} of {context}"
    );
    let mut left = taken;
    let mut fields = [
        usage.cache_read,
        usage.cache_write_1h,
        usage.cache_write_5m,
        usage.input,
    ];
    for field in &mut fields {
        let cut = left.min(*field);
        let (Some(kept), Some(rest)) = (field.checked_sub(cut), left.checked_sub(cut)) else {
            unreachable!("a cut is at most the field and what is left to take")
        };
        *field = kept;
        left = rest;
    }
    assert_eq!(left, 0, "the four fields hold the whole drop");
    let [read, hour_write, five_minute_write, input] = fields;
    Usage {
        input,
        cache_read: read,
        cache_write_5m: five_minute_write,
        cache_write_1h: hour_write,
        ..*usage
    }
}

/// What a call's input, cache reads, and cache writes cost.
fn input_side(priced: &Cost) -> u128 {
    [priced.input, priced.cache_read, priced.cache_write()]
        .into_iter()
        .fold(0_u128, |sum, part| add(sum, u128::from(part)))
}
