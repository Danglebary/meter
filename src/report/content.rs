//! What a transcript record adds to the context: the tool calls an assistant record makes,
//! and the tool results, text, and attachments a user or attachment record adds, each
//! measured in characters. A character is a Unicode scalar value, the unit jq's `length`
//! counts, which the baseline's extract measured in.

use serde_json::Value;

/// The characters an image block counts as.
pub const IMAGE_CHARACTERS: u64 = 6_000;
const SKILL_BODY_OPENING: &str = "Base directory for this skill";
const SLASH_COMMAND_MARK: &str = "<command-name>";
const PICKUP_COMMAND_MARK: &str = "<command-name>/pirr:pickup</command-name>";
const HARNESS_OPENINGS: [&str; 3] = ["<system-reminder", "<local-command", "<task-notification"];
const BASH_TOOL: &str = "Bash";
const DIRECTORY_CHANGE: &str = "cd ";

/// A tool call an assistant record makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    /// The program a Bash call runs, where the call is Bash's and names one.
    pub program: Option<String>,
}

/// One thing a record adds to the context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    ToolResult {
        tool_use_id: String,
        characters: u64,
    },
    Text {
        class: TextClass,
        characters: u64,
    },
}

/// What a piece of text added to the context is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextClass {
    /// The body of a skill Claude Code loads into the conversation.
    SkillBody,
    /// A slash command the user typed, with its expansion.
    SlashCommand,
    /// Text the harness adds, such as a system reminder or a task notification.
    HarnessText,
    /// Text the user typed.
    UserTyped,
    /// An attachment record, named by its type.
    Attachment(String),
}

impl TextClass {
    /// The class as a report names it.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Self::SkillBody => "skill body".to_owned(),
            Self::SlashCommand => "slash command".to_owned(),
            Self::HarnessText => "harness text".to_owned(),
            Self::UserTyped => "typed by the user".to_owned(),
            Self::Attachment(kind) => format!("attachment: {kind}"),
        }
    }
}

/// The turn a user record opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// A prompt running `/pirr:pickup`.
    Pickup,
    /// Any other prompt: text the user typed, or another slash command.
    Other,
}

/// The tool calls in an assistant record's content.
#[must_use]
pub fn tool_uses(record: &Value) -> Vec<ToolUse> {
    let Some(content) = record.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|block| block.pointer("/type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| {
            let id = block.pointer("/id").and_then(Value::as_str)?;
            let name = block.pointer("/name").and_then(Value::as_str)?;
            let command = block.pointer("/input/command").and_then(Value::as_str);
            let program = if name == BASH_TOOL {
                command.and_then(program)
            } else {
                None
            };
            Some(ToolUse {
                id: id.to_owned(),
                name: name.to_owned(),
                program,
            })
        })
        .collect()
}

/// What a user record adds: its tool results and its text.
#[must_use]
pub fn user_items(record: &Value) -> Vec<Item> {
    match record.pointer("/message/content") {
        Some(Value::String(text)) => vec![text_item(text)],
        Some(Value::Array(blocks)) => blocks.iter().filter_map(block_item).collect(),
        _ => Vec::new(),
    }
}

fn block_item(block: &Value) -> Option<Item> {
    match block.pointer("/type").and_then(Value::as_str) {
        Some("tool_result") => {
            let tool_use_id = block.pointer("/tool_use_id").and_then(Value::as_str)?;
            let content = block.pointer("/content").unwrap_or(&Value::Null);
            Some(Item::ToolResult {
                tool_use_id: tool_use_id.to_owned(),
                characters: content_characters(content),
            })
        }
        Some("text") => {
            let text = block.pointer("/text").and_then(Value::as_str)?;
            Some(text_item(text))
        }
        _ => None,
    }
}

/// What an attachment record adds: the attachment, measured as its JSON text.
#[must_use]
pub fn attachment_item(record: &Value) -> Option<Item> {
    let attachment = record.pointer("/attachment")?;
    let kind = attachment
        .pointer("/type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Some(Item::Text {
        class: TextClass::Attachment(kind.to_owned()),
        characters: json_characters(attachment),
    })
}

fn text_item(text: &str) -> Item {
    Item::Text {
        class: class(text),
        characters: characters(text),
    }
}

/// The prompt a user record makes, where it makes one: text the user typed or a slash
/// command opens a turn, and harness text and skill bodies open none.
#[must_use]
pub fn prompt(record: &Value) -> Option<Prompt> {
    let texts: Vec<&str> = match record.pointer("/message/content") {
        Some(Value::String(text)) => vec![text.as_str()],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.pointer("/type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.pointer("/text").and_then(Value::as_str))
            .collect(),
        _ => Vec::new(),
    };
    let mut made = None;
    for text in texts {
        match class(text) {
            TextClass::SlashCommand => {
                if text.contains(PICKUP_COMMAND_MARK) {
                    return Some(Prompt::Pickup);
                }
                made = Some(Prompt::Other);
            }
            TextClass::UserTyped => made = Some(Prompt::Other),
            TextClass::SkillBody | TextClass::HarnessText => {}
            TextClass::Attachment(_) => unreachable!("a user record's text is no attachment"),
        }
    }
    made
}

/// What a piece of user-record text is, by how it opens or what it holds.
#[must_use]
pub fn class(text: &str) -> TextClass {
    if text.starts_with(SKILL_BODY_OPENING) {
        TextClass::SkillBody
    } else if text.contains(SLASH_COMMAND_MARK) {
        TextClass::SlashCommand
    } else if HARNESS_OPENINGS
        .iter()
        .any(|opening| text.starts_with(opening))
    {
        TextClass::HarnessText
    } else {
        TextClass::UserTyped
    }
}

/// The characters a tool result's content holds: a string's, or the sum of its blocks', a
/// text block counting its text, an image block `IMAGE_CHARACTERS`, and any other block its
/// JSON text.
#[must_use]
pub fn content_characters(content: &Value) -> u64 {
    match content {
        Value::String(text) => characters(text),
        Value::Array(blocks) => blocks.iter().fold(0_u64, |sum, block| {
            let counted = match block.pointer("/type").and_then(Value::as_str) {
                Some("text") => block
                    .pointer("/text")
                    .and_then(Value::as_str)
                    .map_or(0, characters),
                Some("image") => IMAGE_CHARACTERS,
                _ => json_characters(block),
            };
            let Some(total) = sum.checked_add(counted) else {
                unreachable!("the characters of a bounded line fit a u64")
            };
            total
        }),
        other => json_characters(other),
    }
}

fn json_characters(value: &Value) -> u64 {
    characters(&value.to_string())
}

fn characters(text: &str) -> u64 {
    let Ok(count) = u64::try_from(text.chars().count()) else {
        unreachable!("the characters of a bounded line fit a u64")
    };
    count
}

/// The program a Bash command line runs: the first word of its first line that is no shell
/// variable assignment, after a leading `cd` into a directory joined by `&&` or `;`.
#[must_use]
pub fn program(command: &str) -> Option<String> {
    let trimmed = command.trim_start();
    let after_cd = if trimmed.starts_with(DIRECTORY_CHANGE) {
        let joint = trimmed
            .char_indices()
            .find(|&(_, letter)| letter == '&' || letter == ';')
            .map(|(index, _)| index);
        match joint.and_then(|index| trimmed.get(index..)) {
            Some(rest) => rest.trim_start_matches(['&', ';']).trim_start(),
            None => trimmed,
        }
    } else {
        trimmed
    };
    let first_line = after_cd.lines().next()?;
    let word = first_line
        .split_whitespace()
        .find(|word| !is_assignment(word))?;
    Some(word.trim_end_matches(';').to_owned())
}

/// Whether a word assigns a shell variable: a name of letters, digits, and underscores, not
/// opening with a digit, then `=`.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let opens_with_letter = name
        .chars()
        .next()
        .is_some_and(|letter| letter.is_ascii_alphabetic() || letter == '_');
    let named = name
        .chars()
        .all(|letter| letter.is_ascii_alphanumeric() || letter == '_');
    if opens_with_letter { named } else { false }
}
