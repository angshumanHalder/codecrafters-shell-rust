mod commands;

#[allow(unused_imports)]
use std::io::{self, Write};

use rustyline::completion::{Completer, Pair};
use rustyline::{Editor, Result, error::ReadlineError};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};

const BUILTINS: &[&str] = &["exit", "echo", "pwd", "type", "cd"];

#[derive(Helper, Hinter, Highlighter, Validator)]
struct ReplHelper;

impl Completer for ReplHelper {
    type Candidate = Pair;
    fn complete(
        &self, // FIXME should be `&mut self`
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> Result<(usize, Vec<Self::Candidate>)> {
        let mut candidates: Vec<Pair> = Vec::new();
        for cmd in BUILTINS {
            if cmd.starts_with(&line[0..pos]) {
                candidates.push(Pair {
                    display: cmd.to_string(),
                    replacement: format!("{} ", cmd.to_string()),
                });
            }
        }
        Ok((0, candidates))
    }
}

fn main() -> Result<()> {
    repl()
}

fn repl() -> Result<()> {
    let mut rl = Editor::new()?;
    rl.set_helper(Some(ReplHelper));
    loop {
        let readline = rl.readline("$ ");
        match readline {
            Ok(line) => {
                commands::process_command(&line);
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }
    Ok(())
}
