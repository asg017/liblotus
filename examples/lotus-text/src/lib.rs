//! Text-based spreadsheet format and tooling for `lotus-core`.
//!
//! - [`format`] parses and emits the line-based `.sheet` DSL.
//! - [`snapshot`] extends that format with ` => computed` tails for
//!   snapshot testing.
//! - [`lint`] reports syntactic, reference, and evaluation diagnostics.
//! - [`repl`] exposes a line-oriented REPL core; the `lotus-repl` binary
//!   is a thin stdin/stdout wrapper over it.

pub mod format;
pub mod highlight;
pub mod lint;
pub mod repl;
pub mod snapshot;

pub use format::{format_compact, parse, ParseError, TextSheet};
pub use lint::{lint, lint_sheet, Diagnostic, Severity};
pub use snapshot::{format_snapshot, verify_snapshot, SnapshotMismatch, VerifyError};
