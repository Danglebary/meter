//! What carrying each item in a thread's context cost. An item's characters over four are
//! its tokens, and every later call of its transcript re-reads them from the cache at that
//! call's own cache-read rate; the model's output is carried the same way in its own tokens.

use std::collections::BTreeMap;

use super::content::TextClass;
use super::price::cache_read_rate;
use super::range::Thread;
use super::tally::{add, plus};
use super::transcript::{Call, Transcript};

/// The characters taken as one token.
pub const CHARACTERS_PER_TOKEN: u128 = 4;
const READ_TOOL: &str = "Read";

/// An item a transcript added to the context, placed among its calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub item: Carried,
    pub characters: u64,
    /// The calls of the transcript that began before the item was added, in file order.
    pub calls_before: usize,
}

/// What an item carried in the context is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carried {
    ToolResult {
        tool: String,
        program: Option<String>,
    },
    Text(TextClass),
}

/// The items of one kind carried in a context, and what their re-reads cost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Carry {
    pub items: u64,
    pub characters: u128,
    /// Each item's characters times the cache-read rates, in nanodollars per token, of
    /// the calls after it, summed: the re-reads' cost times `CHARACTERS_PER_TOKEN`.
    pub weighted: u128,
}

impl Carry {
    /// The items' tokens, their characters over `CHARACTERS_PER_TOKEN`, rounded down.
    #[must_use]
    pub fn tokens(&self) -> u128 {
        floor_quarter(self.characters)
    }

    /// What the re-reads cost, in nanodollars, rounded down.
    #[must_use]
    pub fn nanodollars(&self) -> u128 {
        floor_quarter(self.weighted)
    }

    #[must_use]
    pub fn plus(self, more: Self) -> Self {
        Self {
            items: plus(self.items, more.items),
            characters: add(self.characters, more.characters),
            weighted: add(self.weighted, more.weighted),
        }
    }
}

/// The model's output carried in the context, and the part of it that was thinking.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutputCarry {
    pub tokens: u128,
    pub nanodollars: u128,
    pub thinking_tokens: u128,
    pub thinking_nanodollars: u128,
}

impl OutputCarry {
    #[must_use]
    fn plus(self, more: Self) -> Self {
        Self {
            tokens: add(self.tokens, more.tokens),
            nanodollars: add(self.nanodollars, more.nanodollars),
            thinking_tokens: add(self.thinking_tokens, more.thinking_tokens),
            thinking_nanodollars: add(self.thinking_nanodollars, more.thinking_nanodollars),
        }
    }
}

/// What one transcript carried in its context.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Carriage {
    pub tools: BTreeMap<String, Carry>,
    /// Bash results, by the program each command ran.
    pub commands: BTreeMap<String, Carry>,
    pub texts: BTreeMap<TextClass, Carry>,
    pub output: OutputCarry,
}

/// What `placed` items and the output of `calls`, the transcript's calls in file order,
/// cost to carry.
#[must_use]
pub fn carriage(placed: &[Placed], calls: &[Call]) -> Carriage {
    let rates_after = rates_after(calls);
    let mut carriage = Carriage::default();
    for item in placed {
        let Some(&rate) = rates_after.get(item.calls_before) else {
            unreachable!("an item follows at most every call: {}", item.calls_before)
        };
        let characters = u128::from(item.characters);
        let carry = Carry {
            items: 1,
            characters,
            weighted: product(characters, rate),
        };
        match &item.item {
            Carried::ToolResult { tool, program } => {
                let by_tool = carriage.tools.entry(tool.clone()).or_default();
                *by_tool = by_tool.plus(carry);
                if let Some(program) = program {
                    let by_program = carriage.commands.entry(program.clone()).or_default();
                    *by_program = by_program.plus(carry);
                }
            }
            Carried::Text(class) => {
                let by_class = carriage.texts.entry(class.clone()).or_default();
                *by_class = by_class.plus(carry);
            }
        }
    }
    for (index, call) in calls.iter().enumerate() {
        let Some(&rate) = index.checked_add(1).and_then(|next| rates_after.get(next)) else {
            unreachable!("every call has the calls after it: {index}")
        };
        let output = u128::from(call.usage.output);
        let thinking = u128::from(call.usage.thinking);
        carriage.output = carriage.output.plus(OutputCarry {
            tokens: output,
            nanodollars: product(output, rate),
            thinking_tokens: thinking,
            thinking_nanodollars: product(thinking, rate),
        });
    }
    carriage
}

/// For each count of calls `k`, the cache-read rates of the calls from the `k`-th on,
/// summed: one more entry than there are calls, the last none.
fn rates_after(calls: &[Call]) -> Vec<u128> {
    let mut sums = Vec::with_capacity(calls.len().saturating_add(1));
    let mut sum: u128 = 0;
    sums.push(sum);
    for call in calls.iter().rev() {
        let rate = cache_read_rate(call).unwrap_or(0);
        sum = add(sum, u128::from(rate));
        sums.push(sum);
    }
    sums.reverse();
    assert_eq!(
        sums.len(),
        calls.len().saturating_add(1),
        "one sum for each count of calls"
    );
    sums
}

/// What the context section on what fills it holds: the main threads' carriage, and the
/// subagents' Read results.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FillSection {
    pub main: Carriage,
    pub subagent_reads: Carry,
}

/// The section on what fills the context of the threads in `transcripts`.
#[must_use]
pub fn fill_section(transcripts: &[Transcript]) -> FillSection {
    let mut section = FillSection::default();
    for transcript in transcripts {
        let carriage = &transcript.carriage;
        match transcript.thread {
            Thread::Main => section.main = merged(&section.main, carriage),
            Thread::Subagent => {
                let reads = carriage.tools.get(READ_TOOL).copied().unwrap_or_default();
                section.subagent_reads = section.subagent_reads.plus(reads);
            }
        }
    }
    section
}

/// Two carriages summed.
fn merged(left: &Carriage, right: &Carriage) -> Carriage {
    Carriage {
        tools: merged_maps(&left.tools, &right.tools),
        commands: merged_maps(&left.commands, &right.commands),
        texts: merged_maps(&left.texts, &right.texts),
        output: left.output.plus(right.output),
    }
}

fn merged_maps<K: Ord + Clone>(
    left: &BTreeMap<K, Carry>,
    right: &BTreeMap<K, Carry>,
) -> BTreeMap<K, Carry> {
    // The left map is copied so neither input changes.
    let mut merged = left.clone();
    for (key, carry) in right {
        let entry = merged.entry(key.clone()).or_default();
        *entry = entry.plus(*carry);
    }
    merged
}

fn product(count: u128, rate: u128) -> u128 {
    let Some(product) = count.checked_mul(rate) else {
        unreachable!("bounded characters times bounded rates fit a u128")
    };
    product
}

fn floor_quarter(value: u128) -> u128 {
    let Some(quarter) = value.checked_div(CHARACTERS_PER_TOKEN) else {
        unreachable!("the divisor is a nonzero constant")
    };
    quarter
}
