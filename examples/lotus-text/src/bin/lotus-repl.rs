//! Interactive REPL for `.sheet` files.
//!
//! ```text
//! lotus-repl [file.sheet]
//! ```
//!
//! Behavior lives in `lotus_text::repl::Session`; this binary adds rustyline
//! for history + as-you-type syntax highlighting, and colorizes output when
//! stdout is a TTY.

use std::borrow::Cow;
use std::env;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process;

use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Config, Editor, Helper};

use lotus_text::highlight::{highlight_input, highlight_sheet};
use lotus_text::repl::Session;

struct ReplHelper {
    color: bool,
}

impl Completer for ReplHelper {
    type Candidate = String;
}
impl Hinter for ReplHelper {
    type Hint = String;
}
impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

impl Highlighter for ReplHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if self.color {
            Cow::Owned(highlight_input(line))
        } else {
            Cow::Borrowed(line)
        }
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        // Re-render on every keypress so coloring tracks the input.
        self.color
    }
}

fn main() {
    let mut args = env::args().skip(1);
    let initial = args.next();
    if args.next().is_some() {
        eprintln!("usage: lotus-repl [file.sheet]");
        process::exit(2);
    }

    let color = std::io::stdout().is_terminal();

    let mut session = Session::new();
    if let Some(path) = &initial {
        match std::fs::read_to_string(path) {
            Ok(src) => match session.load_source(&src) {
                Ok(n) => println!("loaded {n} cell(s) from {path}"),
                Err(e) => {
                    eprintln!("parse error: {e}");
                    process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("error reading {path}: {e}");
                process::exit(1);
            }
        }
    }

    let mut rl: Editor<ReplHelper, rustyline::history::FileHistory> =
        match Editor::with_config(Config::builder().auto_add_history(true).build()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("failed to initialize readline: {e}");
                process::exit(1);
            }
        };
    rl.set_helper(Some(ReplHelper { color }));

    let history_path = history_file();
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    loop {
        match rl.readline("> ") {
            Ok(line) => {
                let resp = session.handle(&line);
                if !resp.output.is_empty() {
                    let out = if color {
                        colorize_output(&resp.output)
                    } else {
                        resp.output.clone()
                    };
                    println!("{out}");
                }
                if resp.should_quit {
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: discard the current line, stay in the loop.
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("error: {e}");
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }
}

/// Colorize Session output. Outputs from `:show`, `:snap`, `?`, and `:why`
/// all contain `CELL: ...` lines which `highlight_sheet` handles; other
/// lines pass through untouched.
fn colorize_output(s: &str) -> String {
    highlight_sheet(s)
}

fn history_file() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".cache").join("lotus-repl");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("history"))
}
