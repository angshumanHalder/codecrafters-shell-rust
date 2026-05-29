mod commands;

use std::collections::{BTreeSet, HashMap};
use std::fs;
#[allow(unused_imports)]
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rustyline::completion::{Completer, FilenameCompleter, Pair, extract_word};
use rustyline::{Config, Editor, Result};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};

use crate::commands::JobTable;
use crate::commands::{BUILTINS, reap_children};

static COMPLETIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static JOBS: OnceLock<Mutex<JobTable>> = OnceLock::new();

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
            let words: Vec<&str> = line[..pos].split_whitespace().collect();
            let prev_word = if current_word.is_empty() {
                words.last().copied().unwrap_or("")
            } else {
                words
                    .get(words.len().saturating_sub(2))
                    .copied()
                    .unwrap_or("")
            };
            let cmd = line.split_whitespace().next().unwrap_or("");
            if let Some(completer_script) = get_completions().lock().unwrap().get(cmd) {
                let output = std::process::Command::new(&completer_script)
                    .arg(cmd)
                    .arg(current_word)
                    .arg(prev_word)
                    .env("COMP_LINE", line)
                    .env("COMP_POINT", pos.to_string())
                    .output();
                let mut candidates: Vec<Pair> = Vec::new();
                if let Ok(out) = output {
                    candidates = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(|l| Pair {
                            display: l.to_string(),
                            replacement: format!("{} ", l),
                        })
                        .collect();
                }
                return Ok((start_idx, candidates));
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
        reap_children();
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

fn get_job_table() -> &'static Mutex<JobTable> {
    JOBS.get_or_init(|| {
        Mutex::new(JobTable {
            jobs: Vec::new(),
            free_list: BTreeSet::new(),
            next_id: 1,
        })
    })
}
