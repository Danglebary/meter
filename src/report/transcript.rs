//! One transcript, read a line at a time: each line is read through the shared transcript
//! parser, the records of one call fold into that call, each tool result is paired with
//! its call through the parser's pairing, and what user and attachment records add to the
//! context is placed among the calls that carry it.

use std::collections::HashMap;

use claude_code_transcript::calls::{self, Awaited, Completed, Paired, Pairing};
use claude_code_transcript::record::{self, Assistant, Attachment, TOKENS_PER_FIELD_MAX, User};
use jiff::Timestamp;

use super::carry::{Carriage, Carried, Placed, carriage};
use super::content::{
    Item, Prompt, attachment_item, bash_program, content_characters, prompt, user_items,
};
use super::range::{Range, Thread};

/// The model Claude Code names on the assistant records it writes itself, such as a notice
/// of an expired login, which reach no model and carry no tokens.
pub const SYNTHETIC_MODEL: &str = "<synthetic>";

/// The tokens one call reports.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    /// The part of `output` the model spent thinking.
    pub thinking: u64,
}

impl Usage {
    /// The context the call sent: its uncached input, the cache read, and the cache
    /// written.
    #[must_use]
    pub fn context(&self) -> u64 {
        let fields = [
            self.input,
            self.cache_read,
            self.cache_write_5m,
            self.cache_write_1h,
        ];
        let mut context: u64 = 0;
        for field in fields {
            let Some(sum) = context.checked_add(field) else {
                unreachable!("four fields of at most {TOKENS_PER_FIELD_MAX} tokens fit a u64")
            };
            context = sum;
        }
        context
    }
}

/// One assistant record carrying usage. Claude Code writes a call's record once for each
/// of its content blocks, every copy carrying the call's message id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub message_id: String,
    pub timestamp: Timestamp,
    pub model: String,
    pub effort: Option<String>,
    pub usage: Usage,
}

/// What one line of a transcript holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// An assistant record carrying usage, beside the tool calls it makes.
    Record {
        record: Record,
        tool_calls: Vec<calls::Call>,
    },
    /// What a user or attachment record adds to the context.
    Added {
        timestamp: Timestamp,
        items: Vec<Item>,
        /// The turn a user record opens, where it opens one.
        prompt: Option<Prompt>,
    },
    /// A compaction boundary, where Claude Code replaced the context with a summary.
    Compaction { timestamp: Timestamp },
    /// Any line that is neither.
    Other,
    /// A record whose field fails its check; the JSON pointer names the field, or the root
    /// where the line is not JSON.
    Malformed(&'static str),
}

/// Reads one line of a transcript, its line end left off.
///
/// # Panics
///
/// Where `text` holds a line end, which the shared parser refuses.
#[must_use]
pub fn read_line(text: &str) -> Line {
    let read = record::read_line(text);
    match read {
        record::Line::Assistant(assistant) => assistant_line(assistant),
        record::Line::User(user) => user_line(user),
        record::Line::Attachment(attachment) => attachment_line(&attachment),
        record::Line::Compaction(compaction) => Line::Compaction {
            timestamp: compaction.envelope.timestamp,
        },
        record::Line::Other => Line::Other,
        record::Line::Malformed(pointer) => Line::Malformed(pointer),
    }
}

fn assistant_line(assistant: Assistant) -> Line {
    let tool_calls = calls::calls_of(&assistant);
    let usage = usage_of(assistant.usage);
    let record = Record {
        message_id: assistant.message_id,
        timestamp: assistant.envelope.timestamp,
        model: assistant.model,
        effort: assistant.effort,
        usage,
    };
    Line::Record { record, tool_calls }
}

/// The tokens a call reports, field for field.
const fn usage_of(usage: record::Usage) -> Usage {
    let record::Usage {
        input,
        cache_read,
        cache_write_5m,
        cache_write_1h,
        output,
        thinking,
    } = usage;
    Usage {
        input,
        cache_read,
        cache_write_5m,
        cache_write_1h,
        output,
        thinking,
    }
}

/// A user record's items and the turn it opens, placed at its time; a record adding
/// nothing is no record the report reads.
fn user_line(user: User) -> Line {
    let opened = prompt(&user.blocks);
    let timestamp = user.envelope.timestamp;
    let items = user_items(user.blocks);
    if items.is_empty() {
        assert!(opened.is_none(), "a prompt adds its text to the context");
        return Line::Other;
    }
    Line::Added {
        timestamp,
        items,
        prompt: opened,
    }
}

fn attachment_line(attachment: &Attachment) -> Line {
    let item = attachment_item(attachment);
    Line::Added {
        timestamp: attachment.envelope.timestamp,
        items: vec![item],
        prompt: None,
    }
}

/// What a result paired with its call carries: the call's tool and a Bash call's program,
/// and the result's characters.
fn result_carried(paired: Paired) -> (Carried, u64) {
    let program = bash_program(&paired.call.tool_use);
    let characters = content_characters(&paired.result.parts);
    let carried = Carried::ToolResult {
        tool: paired.call.tool_use.name,
        program,
    };
    (carried, characters)
}

/// One call to a model: the time, model, and effort of its first record, and the usage of
/// its last, since a call's output count grows across its records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub timestamp: Timestamp,
    pub model: String,
    pub effort: Option<String>,
    pub usage: Usage,
}

/// A transcript's calls in the order they began, beside what was left out of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub session: String,
    pub thread: Thread,
    pub calls: Vec<Call>,
    /// Calls whose model is `SYNTHETIC_MODEL`, left out of `calls`.
    pub synthetic: u64,
    pub malformed: u64,
    /// The last call of the transcript that began before the range starts, where one did,
    /// so the first of `calls` is not the transcript's first.
    pub before_range: Option<Prior>,
    /// The subagent's type, as its meta file names it; none for a main thread, or for a
    /// subagent whose type is unknown.
    pub agent_type: Option<String>,
    /// What the transcript's context carried, and what its re-reads cost.
    pub carriage: Carriage,
    /// The turn the transcript's first prompt opens, where that prompt runs `/pirr:pickup`.
    pub pickup_turn: Option<Turn>,
    /// When each compaction boundary the transcript records fell, earliest first.
    pub compactions: Vec<Timestamp>,
}

/// A turn of a transcript: from its prompt up to the next prompt, where one followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub start: Timestamp,
    pub end: Option<Timestamp>,
}

impl Turn {
    /// Whether a call beginning at `timestamp` falls in the turn.
    #[must_use]
    pub fn holds(&self, timestamp: Timestamp) -> bool {
        let started = timestamp >= self.start;
        let unended = self.end.is_none_or(|end| timestamp < end);
        if started { unended } else { false }
    }
}

/// A call outside the calls measured, kept for when it began and which model it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prior {
    pub timestamp: Timestamp,
    pub model: String,
}

/// The fold of one transcript's lines into its calls. Records outside the range are
/// skipped one by one, as a cutoff on record timestamps does.
#[derive(Debug)]
pub struct Fold {
    range: Range,
    calls: Vec<Call>,
    call_index: HashMap<String, usize>,
    malformed: u64,
    before_range: Option<Prior>,
    /// The tool calls awaiting their results.
    pairing: Pairing,
    placed: Vec<Placed>,
    /// The transcript's first prompt, and when its second began.
    first_prompt: Option<(Timestamp, Prompt)>,
    second_prompt: Option<Timestamp>,
    compactions: Vec<Timestamp>,
}

impl Fold {
    #[must_use]
    pub fn new(range: Range) -> Self {
        Self {
            range,
            calls: Vec::new(),
            call_index: HashMap::new(),
            malformed: 0,
            before_range: None,
            pairing: Pairing::new(),
            placed: Vec::new(),
            first_prompt: None,
            second_prompt: None,
            compactions: Vec::new(),
        }
    }

    /// Takes the transcript's next line.
    pub fn take(&mut self, line: Line) {
        match line {
            Line::Other => {}
            Line::Malformed(_) => {
                let Some(malformed) = self.malformed.checked_add(1) else {
                    unreachable!("a transcript's lines are counted in a u64")
                };
                self.malformed = malformed;
            }
            Line::Record { record, tool_calls } => {
                for tool_call in tool_calls {
                    self.await_call(tool_call);
                }
                if self.range.holds(record.timestamp) {
                    self.take_record(record);
                } else if self.range.precedes(record.timestamp) {
                    self.take_prior(record);
                }
            }
            Line::Added {
                timestamp,
                items,
                prompt,
            } => {
                if let Some(prompt) = prompt {
                    self.take_prompt(timestamp, prompt);
                }
                // A result outside the range still completes its call, so the calls
                // awaiting are those whose results have not arrived.
                let held = self.range.holds(timestamp);
                for item in items {
                    let (carried, characters) = match item {
                        Item::ToolResult(result) => match self.pairing.complete(result) {
                            Completed::Paired(paired) => result_carried(paired),
                            // A result completing no awaited call is left out.
                            Completed::Unawaited(_) => continue,
                        },
                        Item::Text { class, characters } => (Carried::Text(class), characters),
                    };
                    if held {
                        self.place(carried, characters);
                    }
                }
            }
            Line::Compaction { timestamp } => self.compactions.push(timestamp),
        }
    }

    /// Keeps the first prompt, and when the second began; later prompts open no turn the
    /// report reads.
    const fn take_prompt(&mut self, timestamp: Timestamp, prompt: Prompt) {
        if self.first_prompt.is_none() {
            self.first_prompt = Some((timestamp, prompt));
        } else if self.second_prompt.is_none() {
            self.second_prompt = Some(timestamp);
        }
    }

    /// Keeps the latest call before the range, a synthetic one aside.
    fn take_prior(&mut self, record: Record) {
        let synthetic = record.model == SYNTHETIC_MODEL;
        let later = self
            .before_range
            .as_ref()
            .is_none_or(|prior| record.timestamp >= prior.timestamp);
        if !synthetic {
            if later {
                self.before_range = Some(Prior {
                    timestamp: record.timestamp,
                    model: record.model,
                });
            }
        }
    }

    /// Holds a tool call until its result arrives.
    fn await_call(&mut self, tool_call: calls::Call) {
        let awaited = self.pairing.await_call(tool_call);
        match awaited {
            // A copy of an awaited call leaves the first to take the result, and the
            // result of a call past the pairing's bound completes no call.
            Awaited::Held | Awaited::Duplicate(_) | Awaited::Full(_) => {}
        }
    }

    /// Places what an item carries after the calls taken so far.
    fn place(&mut self, carried: Carried, characters: u64) {
        self.placed.push(Placed {
            item: carried,
            characters,
            calls_before: self.calls.len(),
        });
    }

    fn take_record(&mut self, record: Record) {
        if let Some(&index) = self.call_index.get(&record.message_id) {
            let Some(call) = self.calls.get_mut(index) else {
                unreachable!("the call index names a call the fold holds: {index}")
            };
            call.usage = record.usage;
        } else {
            let index = self.calls.len();
            self.call_index.insert(record.message_id, index);
            self.calls.push(Call {
                timestamp: record.timestamp,
                model: record.model,
                effort: record.effort,
                usage: record.usage,
            });
        }
        assert_eq!(
            self.calls.len(),
            self.call_index.len(),
            "every call has one index entry"
        );
    }

    /// The transcript's calls in the order they began, its synthetic calls counted apart.
    ///
    /// # Panics
    ///
    /// Where the fold has lost a call, which its own bookkeeping excludes.
    #[must_use]
    pub fn finish(self, session: String, thread: Thread) -> Transcript {
        let total = self.calls.len();
        // The carriage reads the calls in file order, the order the items were placed in.
        let carriage = carriage(&self.placed, &self.calls);
        let (synthetic, mut calls): (Vec<Call>, Vec<Call>) = self
            .calls
            .into_iter()
            .partition(|call| call.model == SYNTHETIC_MODEL);
        assert_eq!(
            synthetic.len().checked_add(calls.len()),
            Some(total),
            "the partition keeps every call"
        );
        calls.sort_by_key(|call| call.timestamp);
        let pickup_turn = match self.first_prompt {
            Some((start, Prompt::Pickup)) => Some(Turn {
                start,
                end: self.second_prompt,
            }),
            Some((_, Prompt::Other)) | None => None,
        };
        let mut compactions = self.compactions;
        compactions.sort_unstable();
        let Ok(synthetic) = u64::try_from(synthetic.len()) else {
            unreachable!("a count of calls fits a u64")
        };
        Transcript {
            session,
            thread,
            calls,
            synthetic,
            malformed: self.malformed,
            before_range: self.before_range,
            agent_type: None,
            carriage,
            pickup_turn,
            compactions,
        }
    }
}
