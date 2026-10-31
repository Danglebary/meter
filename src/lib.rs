//! The functional core of meter: the argument grammar, every message, and the exit
//! codes. Nothing here reads or writes a file, reads the environment, or starts a
//! process, so each rule is tested against values; `main.rs` is the shell that does all
//! of that.

pub mod cli;
pub mod exit_code;
pub mod message;
pub mod report;
pub mod statusline;
