//! One transcript, read a line at a time: each assistant record carrying usage is read
//! field by field, and the records of one call fold into that call.

use std::collections::HashMap;

use jiff::Timestamp;
use serde_json::Value;

use super::range::{Range, Thread};

/// The model Claude Code names on the assistant records it writes itself, such as a notice
/// of an expired login, which reach no model and carry no tokens.
pub const SYNTHETIC_MODEL: &str = "<synthetic>";
/// The most tokens one usage field is read as holding; a field above it is malformed. A
/// context window holds a small fraction of this.
pub const TOKENS_PER_FIELD_MAX: u64 = 10_000_000;

/// The tokens one call reports.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
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
    Record(Record),
    /// Any line that is not an assistant record carrying usage.
    Other,
    /// An assistant record whose field fails its check; the JSON pointer names the field,
    /// or the root where the line is not JSON.
    Malformed(&'static str),
}

/// Reads one line of a transcript.
#[must_use]
pub fn read_line(text: &str) -> Line {
    // An assistant record holds this text in its `type` field, so a line without it is
    // no record and skips the parse.
    if !text.contains("\"assistant\"") {
        return Line::Other;
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Line::Malformed("");
    };
    let assistant = value.pointer("/type").and_then(Value::as_str) == Some("assistant");
    let identified = value.pointer("/message/id").is_some_and(|id| !id.is_null());
    let metered = value
        .pointer("/message/usage")
        .is_some_and(|usage| !usage.is_null());
    if assistant {
        if identified {
            if metered {
                return match read_record(&value) {
                    Ok(record) => Line::Record(record),
                    Err(pointer) => Line::Malformed(pointer),
                };
            }
        }
    }
    Line::Other
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
    };
    Ok(Record {
        message_id,
        timestamp,
        model,
        effort,
        usage,
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
                let synthetic = record.model == SYNTHETIC_MODEL;
                if self.range.holds(record.timestamp) {
                    self.take_record(record);
                } else if self.range.precedes(record.timestamp) {
                    if !synthetic {
                        let later = self
                            .before_range
                            .as_ref()
                            .is_none_or(|prior| record.timestamp >= prior.timestamp);
                        if later {
                            self.before_range = Some(Prior {
                                timestamp: record.timestamp,
                                model: record.model,
                            });
                        }
                    }
                }
            }
        }
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
        }
    }
}
