//! The subagent section: each type of subagent, how many ran, what they cost, and what one
//! of them cost, ran, started with, and peaked at on average.

use std::collections::BTreeMap;

use serde_json::Value;

use super::price::cost;
use super::range::Thread;
use super::tally::{add, count_one};
use super::transcript::Transcript;

/// The subagent section, by type; a subagent whose type is unknown falls under `None`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SubagentSection {
    pub by_type: BTreeMap<Option<String>, Agents>,
}

/// The subagents of one type holding a call in the range, with sums over them. Every
/// amount is in nanodollars; a call the price table lacks costs nothing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Agents {
    pub agents: u64,
    pub nanodollars: u128,
    pub calls: u128,
    /// The context of each subagent's first call, summed.
    pub starting_context: u128,
    /// The largest context of each subagent's calls, summed.
    pub peak_context: u128,
}

impl Agents {
    /// The mean of a sum over these agents, rounded half up.
    ///
    /// # Panics
    ///
    /// Where there are no agents, which the section's grouping excludes.
    #[must_use]
    pub fn mean(&self, sum: u128) -> u128 {
        let agents = u128::from(self.agents);
        assert!(agents > 0, "a type groups at least one agent");
        let Some(half) = agents.checked_div(2) else {
            unreachable!("the divisor is a nonzero constant")
        };
        let rounded = add(sum, half);
        let Some(mean) = rounded.checked_div(agents) else {
            unreachable!("the agents counted are nonzero")
        };
        mean
    }
}

/// The type a subagent's meta file names, where it is a JSON object naming one.
#[must_use]
pub fn agent_type(meta: &str) -> Option<String> {
    let value: Value = serde_json::from_str(meta).ok()?;
    match value.pointer("/agentType") {
        Some(Value::String(name)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// The subagent section of the subagent transcripts in `transcripts`.
#[must_use]
pub fn subagent_section(transcripts: &[Transcript]) -> SubagentSection {
    let mut section = SubagentSection::default();
    let subagents = transcripts
        .iter()
        .filter(|transcript| transcript.thread == Thread::Subagent);
    for transcript in subagents {
        let Some(first) = transcript.calls.first() else {
            continue;
        };
        let mut nanodollars: u128 = 0;
        let mut peak: u64 = 0;
        for call in &transcript.calls {
            let priced = cost(&call.model, &call.usage).unwrap_or_default();
            nanodollars = add(nanodollars, u128::from(priced.total()));
            peak = peak.max(call.usage.context());
        }
        let Ok(calls) = u128::try_from(transcript.calls.len()) else {
            unreachable!("a count of calls fits a u128")
        };
        let group = section
            .by_type
            .entry(transcript.agent_type.clone())
            .or_default();
        *group = Agents {
            agents: count_one(group.agents),
            nanodollars: add(group.nanodollars, nanodollars),
            calls: add(group.calls, calls),
            starting_context: add(group.starting_context, u128::from(first.usage.context())),
            peak_context: add(group.peak_context, u128::from(peak)),
        };
    }
    section
}
