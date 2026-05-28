mod commands;

use std::collections::HashMap;
use std::fs;
#[allow(unused_imports)]
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rustyline::completion::{Completer, FilenameCompleter, Pair, extract_word};
use rustyline::{Config, Editor, Result};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};

use crate::commands::BUILTINS;

static COMPLETIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

#[derive(Helper, Hinter, Highlighter, Validator)]
struct ShellHelper {
    #[rustyline(Completer)]
    file_completer: FilenameCompleter,
}

impl Completer for ShellHelper {
    type Candidate = Pair;
    fn complete(
        &self, // FIXME should be `&mut self`
        line: &str,
        pos: usize,
        ctx: &rustyline::Context<'_>,
    ) -> Result<(usize, Vec<Self::Candidate>)> {
        let (start_idx, current_word) = extract_word(line, pos, None, is_break_char);
        let is_cmd_completion = line[..start_idx].trim().is_empty();
        if is_cmd_completion {
            let matches = find_completions(current_word);
            Ok((start_idx, matches))
        } else {
            let (start, mut file_matches) = self.file_completer.complete(line, pos, ctx)?;
            let is_multiple = file_matches.len() > 1;
            for p in &mut file_matches {
                let path = Path::new(&p.replacement);
                if path.is_file() {
                    p.replacement.push(' ');
                } else if path.is_dir() && is_multiple {
                    p.display.push(std::path::MAIN_SEPARATOR);
                }
            }
            Ok((start, file_matches))
        }
    }
}

fn main() -> Result<()> {
    repl()
}

fn repl() -> Result<()> {
    let config = Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .build();
    let mut rl = Editor::with_config(config)?;
    let shell_helper = ShellHelper {
        file_completer: FilenameCompleter::new(),
    };
    rl.set_helper(Some(shell_helper));
    loop {
        let readline = rl.readline("$ ");
        match readline {
            Ok(line) => {
                commands::process_command(&line.trim());
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
        .chain(
            std::env::split_paths(&full_path)
                .flat_map(|dir| fs::read_dir(dir).ok().into_iter().flatten())
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.metadata()
                        .map(|m| m.permissions().mode() & 0o111 != 0)
                        .unwrap_or(false)
                })
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|name| name.starts_with(prefix)),
        )
        .collect();

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

fn is_break_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '"' | '\'')
}

fn get_completions() -> &'static Mutex<HashMap<String, String>> {
    COMPLETIONS.get_or_init(|| Mutex::new(HashMap::new()))
}
