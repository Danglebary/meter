//! One transcript, read a line at a time: each assistant record carrying usage is read
//! field by field, the records of one call fold into that call, and what user and
//! attachment records add to the context is placed among the calls that carry it.

use std::collections::HashMap;

use jiff::Timestamp;
use serde_json::Value;

use super::carry::{Carriage, Carried, Placed, carriage};
use super::content::{Item, Prompt, ToolUse, attachment_item, prompt, tool_uses, user_items};
use super::range::{Range, Thread};

/// The model Claude Code names on the assistant records it writes itself, such as a notice
/// of an expired login, which reach no model and carry no tokens.
pub const SYNTHETIC_MODEL: &str = "<synthetic>";
/// The most tokens one usage field is read as holding; a field above it is malformed. A
/// context window holds a small fraction of this.
pub const TOKENS_PER_FIELD_MAX: u64 = 10_000_000;
/// A user record holds its type in this form, as Claude Code writes JSON without spaces.
const USER_TYPE: &str = "\"type\":\"user\"";
/// An attachment record holds its type in this form.
const ATTACHMENT_TYPE: &str = "\"type\":\"attachment\"";
/// A compaction boundary's system record holds its subtype in this form.
const COMPACT_BOUNDARY_SUBTYPE: &str = "\"subtype\":\"compact_boundary\"";

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
    pub tool_uses: Vec<ToolUse>,
}

/// What one line of a transcript holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Record(Record),
    /// What a user or attachment record adds to the context.
    Added {
        timestamp: Timestamp,
        items: Vec<Item>,
        /// The turn a user record opens, where it opens one.
        prompt: Option<Prompt>,
    },
    /// A compaction boundary, where Claude Code replaced the context with a summary.
    Compaction {
        timestamp: Timestamp,
    },
    /// Any line that is neither.
    Other,
    /// A record whose field fails its check; the JSON pointer names the field, or the root
    /// where the line is not JSON.
    Malformed(&'static str),
}

/// Reads one line of a transcript.
#[must_use]
pub fn read_line(text: &str) -> Line {
    // A record the report reads holds one of these texts, so a line holding none of them
    // skips the parse.
    let assistant = text.contains("\"assistant\"");
    let user = text.contains(USER_TYPE);
    let attachment = text.contains(ATTACHMENT_TYPE);
    let compaction = text.contains(COMPACT_BOUNDARY_SUBTYPE);
    let read = assistant || user || attachment || compaction;
    if !read {
        return Line::Other;
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Line::Malformed("");
    };
    match value.pointer("/type").and_then(Value::as_str) {
        Some("assistant") => read_assistant(&value),
        Some("user") => read_added(&value, user_items(&value), prompt(&value)),
        Some("attachment") => {
            read_added(&value, attachment_item(&value).into_iter().collect(), None)
        }
        Some("system") => read_compaction(&value),
        _ => Line::Other,
    }
}

fn read_assistant(value: &Value) -> Line {
    let identified = value.pointer("/message/id").is_some_and(|id| !id.is_null());
    let metered = value
        .pointer("/message/usage")
        .is_some_and(|usage| !usage.is_null());
    if identified {
        if metered {
            return match read_record(value) {
                Ok(record) => Line::Record(record),
                Err(pointer) => Line::Malformed(pointer),
            };
        }
    }
    Line::Other
}

/// A user or attachment record's `items` and the turn it opens, placed at its time; a
/// record adding nothing is no record the report reads.
fn read_added(value: &Value, items: Vec<Item>, prompt: Option<Prompt>) -> Line {
    if items.is_empty() {
        assert!(prompt.is_none(), "a prompt adds its text to the context");
        return Line::Other;
    }
    let Ok(stamp) = text(value, "/timestamp") else {
        return Line::Malformed("/timestamp");
    };
    match stamp.parse::<Timestamp>() {
        Ok(timestamp) => Line::Added {
            timestamp,
            items,
            prompt,
        },
        Err(_) => Line::Malformed("/timestamp"),
    }
}

/// A system record marking a compaction boundary; any other system record is no record
/// the report reads.
fn read_compaction(value: &Value) -> Line {
    let boundary = value.pointer("/subtype").and_then(Value::as_str) == Some("compact_boundary");
    if !boundary {
        return Line::Other;
    }
    let Ok(stamp) = text(value, "/timestamp") else {
        return Line::Malformed("/timestamp");
    };
    match stamp.parse::<Timestamp>() {
        Ok(timestamp) => Line::Compaction { timestamp },
        Err(_) => Line::Malformed("/timestamp"),
    }
}

fn read_record(value: &Value) -> Result<Record, &'static str> {
    let message_id = text(value, "/message/id")?;
    let stamp = text(value, "/timestamp")?;
    let Ok(timestamp) = stamp.parse::<Timestamp>() else {
        return Err("/timestamp");
    };
    let model = text(value, "/message/model")?;
    let effort = match value.pointer("/effort") {
        None | Some(Value::Null) => None,
        Some(_) => Some(text(value, "/effort")?),
    };
    let usage = Usage {
        input: tokens(value, "/message/usage/input_tokens")?,
        cache_read: tokens(value, "/message/usage/cache_read_input_tokens")?,
        cache_write_5m: tokens(
            value,
            "/message/usage/cache_creation/ephemeral_5m_input_tokens",
        )?,
        cache_write_1h: tokens(
            value,
            "/message/usage/cache_creation/ephemeral_1h_input_tokens",
        )?,
        output: tokens(value, "/message/usage/output_tokens")?,
        thinking: tokens(
            value,
            "/message/usage/output_tokens_details/thinking_tokens",
        )?,
    };
    Ok(Record {
        message_id,
        timestamp,
        model,
        effort,
        usage,
        tool_uses: tool_uses(value),
    })
}

fn text(value: &Value, pointer: &'static str) -> Result<String, &'static str> {
    match value.pointer(pointer) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        _ => Err(pointer),
    }
}

/// A count of tokens; an absent field counts none.
fn tokens(value: &Value, pointer: &'static str) -> Result<u64, &'static str> {
    match value.pointer(pointer) {
        None | Some(Value::Null) => Ok(0),
        Some(field) => match field.as_u64() {
            Some(count) if count <= TOKENS_PER_FIELD_MAX => Ok(count),
            Some(_) | None => Err(pointer),
        },
    }
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
    /// The name, and a Bash call's program, of each tool call, by its id.
    tool_names: HashMap<String, (String, Option<String>)>,
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
            tool_names: HashMap::new(),
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
            Line::Record(record) => {
                for tool_use in &record.tool_uses {
                    let named = (tool_use.name.clone(), tool_use.program.clone());
                    self.tool_names.insert(tool_use.id.clone(), named);
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
                if self.range.holds(timestamp) {
                    for item in items {
                        self.place(item);
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

    /// Places an item after the calls taken so far; a tool result whose call the
    /// transcript never named is left out.
    fn place(&mut self, item: Item) {
        let (carried, characters) = match item {
            Item::ToolResult {
                tool_use_id,
                characters,
            } => {
                let Some((tool, program)) = self.tool_names.get(&tool_use_id) else {
                    return;
                };
                let carried = Carried::ToolResult {
                    tool: tool.clone(),
                    program: program.clone(),
                };
                (carried, characters)
            }
            Item::Text { class, characters } => (Carried::Text(class), characters),
        };
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
