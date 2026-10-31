//! What a transcript record adds to the context: the tool calls an assistant record makes,
//! and the tool results, text, and attachments a user or attachment record adds, each
//! measured in characters. A character is a Unicode scalar value, the unit jq's `length`
//! counts, which the baseline's extract measured in.

use claude_code_transcript::record::{self, AssistantBlock, Attachment, ResultPart, UserBlock};
use serde_json::Value;

/// The characters an image block counts as.
pub const IMAGE_CHARACTERS: u64 = 6_000;
const PICKUP_COMMAND_MARK: &str = "<command-name>/pirr:pickup</command-name>";
/// The tag Claude Code wraps a shell command the user ran with `!` in.
const SHELL_INPUT_TAG: &str = "bash-input";
const BASH_TOOL: &str = "Bash";
const COMMAND_FIELD: &str = "command";
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

/// The tool calls among an assistant record's blocks.
///
/// # Panics
///
/// Where it finds more calls than there are blocks, which no record can cause.
#[must_use]
pub fn tool_uses(blocks: &[AssistantBlock]) -> Vec<ToolUse> {
    let mut tool_uses = Vec::new();
    for block in blocks {
        if let AssistantBlock::ToolUse(tool_use) = block {
            let named = tool_use_of(tool_use);
            tool_uses.push(named);
        }
    }
    assert!(
        tool_uses.len() <= blocks.len(),
        "each tool call is one block"
    );
    tool_uses
}

fn tool_use_of(tool_use: &record::ToolUse) -> ToolUse {
    let command = tool_use.input.get(COMMAND_FIELD).and_then(Value::as_str);
    let program = if tool_use.name == BASH_TOOL {
        command.and_then(program)
    } else {
        None
    };
    ToolUse {
        id: tool_use.id.clone(),
        name: tool_use.name.clone(),
        program,
    }
}

/// What a user record's blocks add: its tool results and its text.
///
/// # Panics
///
/// Where it finds more items than there are blocks, which no record can cause.
#[must_use]
pub fn user_items(blocks: &[UserBlock]) -> Vec<Item> {
    let mut items = Vec::new();
    for block in blocks {
        match block {
            UserBlock::Text { text, class } => items.push(Item::Text {
                class: text_class(class),
                characters: characters(text),
            }),
            UserBlock::ToolResult(result) => items.push(Item::ToolResult {
                tool_use_id: result.tool_use_id.clone(),
                characters: content_characters(&result.parts),
            }),
            UserBlock::Image | UserBlock::Other => {}
        }
    }
    assert!(items.len() <= blocks.len(), "each item is one block");
    items
}

/// What an attachment record adds: the attachment, measured as its JSON text.
#[must_use]
pub fn attachment_item(attachment: &Attachment) -> Item {
    Item::Text {
        class: TextClass::Attachment(attachment.kind.clone()),
        characters: json_characters(&attachment.body),
    }
}

/// The prompt a user record's blocks make, where they make one: text the user typed or a
/// slash command opens a turn, and harness text and skill bodies open none.
#[must_use]
pub fn prompt(blocks: &[UserBlock]) -> Option<Prompt> {
    let mut made = None;
    for block in blocks {
        let UserBlock::Text { text, class } = block else {
            continue;
        };
        match text_class(class) {
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

/// How a report counts text the shared parser classes: typed text, and a shell command the
/// user ran with `!`, as typed by the user; an interruption note and every other tagged
/// text as harness text.
#[must_use]
pub fn text_class(class: &record::TextClass) -> TextClass {
    match class {
        record::TextClass::Typed => TextClass::UserTyped,
        record::TextClass::SkillBody => TextClass::SkillBody,
        record::TextClass::SlashCommand => TextClass::SlashCommand,
        record::TextClass::Interruption => TextClass::HarnessText,
        record::TextClass::Tagged(tag) => {
            if tag == SHELL_INPUT_TAG {
                TextClass::UserTyped
            } else {
                TextClass::HarnessText
            }
        }
    }
}

/// The characters a tool result's parts hold: a text part counting its text, an image part
/// `IMAGE_CHARACTERS`, and any other part its JSON text.
#[must_use]
pub fn content_characters(parts: &[ResultPart]) -> u64 {
    let mut total: u64 = 0;
    for part in parts {
        let counted = match part {
            ResultPart::Text(text) => characters(text),
            ResultPart::Image => IMAGE_CHARACTERS,
            ResultPart::Other(value) => json_characters(value),
        };
        let Some(sum) = total.checked_add(counted) else {
            unreachable!("the characters of a bounded line fit a u64")
        };
        total = sum;
    }
    total
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
