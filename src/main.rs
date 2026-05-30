mod commands;

use std::collections::{BTreeSet, HashMap};
use std::fs;
#[allow(unused_imports)]
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use rustyline::completion::{Completer, FilenameCompleter, Pair, extract_word};
use rustyline::history::History;
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
        .history_ignore_dups(false)?
        .build();
    let mut rl = Editor::with_config(config)?;
    let shell_helper = ShellHelper {
        file_completer: FilenameCompleter::new(),
    };
    rl.set_helper(Some(shell_helper));
    let history_path = std::env::var_os("HISTFILE").unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        let mut p = std::path::PathBuf::from(home);
        p.push(".shell_history");
        p.into_os_string()
    });
    let mut history_append_offset: usize;
    let _ = rl.load_history(&history_path);
    history_append_offset = rl.history().len();
    loop {
        reap_children(true);
        let readline = rl.readline("$ ");
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                let _ = rl.add_history_entry(trimmed);
                if trimmed.starts_with("history") {
                    handle_history(&mut rl, trimmed, &mut history_append_offset);
                } else {
                    commands::process_command(&line.trim());
                }
            }
            Err(_) => {
                break;
            }
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history_path)
    {
        use rustyline::history::SearchDirection;
        for i in 0..rl.history().len() {
            if let Ok(Some(result)) = rl.history().get(i, SearchDirection::Forward) {
                let _ = writeln!(file, "{}", result.entry);
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
            current_job_id: None,
            prev_job_id: None,
        })
    })
}

fn handle_history(
    rl: &mut Editor<ShellHelper, rustyline::history::DefaultHistory>,
    input: &str,
    append_offset: &mut usize,
) {
    use rustyline::history::SearchDirection;
    let args: Vec<&str> = input.split_whitespace().collect();
    match args.get(1).copied() {
        Some("-r") => {
            if let Some(path) = args.get(2) {
                if let Ok(contents) = std::fs::read_to_string(path) {
                    for line in contents.lines() {
                        if !line.is_empty() {
                            let _ = rl.add_history_entry(line);
                        }
                    }
                }
                *append_offset = rl.history().len();
            } else {
                eprintln!("history: -r: missing file operand");
            }
        }
        Some("-w") => {
            if let Some(path) = args.get(2) {
                if let Ok(mut file) = std::fs::File::create(path) {
                    for i in 0..rl.history().len() {
                        if let Ok(Some(result)) = rl.history().get(i, SearchDirection::Forward) {
                            let _ = writeln!(file, "{}", result.entry);
                        }
                    }
                }
            } else {
                eprintln!("history: -w: missing file operand");
            }
        }
        Some("-a") => {
            if let Some(path) = args.get(2) {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let total = rl.history().len();
                    for i in *append_offset..total {
                        if let Ok(Some(result)) = rl.history().get(i, SearchDirection::Forward) {
                            let _ = writeln!(file, "{}", result.entry);
                        }
                    }
                    *append_offset = total;
                }
            } else {
                eprintln!("history: -a: missing file operand");
            }
        }
        _ => {
            let total = rl.history().len();
            let limit = args
                .get(1)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(total);
            let start = total.saturating_sub(limit);
            for i in start..total {
                if let Ok(Some(result)) = rl.history().get(i, SearchDirection::Forward) {
                    println!("{:5}  {}", i + 1, result.entry);
                }
            }
        }
    }
}
