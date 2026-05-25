mod commands;

use std::fs;
#[allow(unused_imports)]
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;

use rustyline::completion::{Completer, Pair};
use rustyline::{Editor, Result};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};

use crate::commands::BUILTINS;

#[derive(Helper, Hinter, Highlighter, Validator)]
struct ShellHelper;

impl Completer for ShellHelper {
    type Candidate = Pair;
    fn complete(
        &self, // FIXME should be `&mut self`
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> Result<(usize, Vec<Self::Candidate>)> {
        let word = &line[0..pos];
        let matches = find_completions(word);
        Ok((0, matches))
    }
}

fn main() -> Result<()> {
    repl()
}

fn repl() -> Result<()> {
    let mut rl = Editor::new()?;
    rl.set_helper(Some(ShellHelper));
    loop {
        let readline = rl.readline("$ ");
        match readline {
            Ok(line) => {
                commands::process_command(&line);
            }
            Err(_) => {
                break;
            }
        }
    }
    Ok(())
}

fn find_completions(prefix: &str) -> Vec<Pair> {
    let full_path = std::env::var_os("PATH").unwrap_or_default();
    let mut matches: Vec<String> = BUILTINS
        .iter()
        .filter(|b| b.starts_with(prefix))
        .map(|b| b.to_string())
        .collect();
    let path_matches: Vec<String> = std::env::split_paths(&full_path)
        .flat_map(|dir| fs::read_dir(dir).ok().into_iter().flatten())
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(prefix))
        .collect();

    matches.extend(path_matches);
    matches.sort();
    matches.dedup();
    matches
        .into_iter()
        .map(|m| Pair {
            display: m.clone(),
            replacement: format!("{} ", m),
        })
        .collect()
}
