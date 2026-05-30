use std::collections::BTreeSet;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process;
use std::process::exit;

use nix::sys::wait::WaitPidFlag;
use nix::sys::wait::WaitStatus;
use nix::sys::wait::waitpid;
use nix::unistd::Pid;

use crate::get_completions;
use crate::get_job_table;
use crate::get_variables;

pub const BUILTINS: &[&str] = &[
    "exit", "echo", "pwd", "type", "cd", "complete", "jobs", "history", "declare",
];

enum CommandKind {
    Builtin,
    External(PathBuf),
    NotFound,
}

#[derive(Debug, PartialEq)]
enum State {
    Default,
    InSingleQuote,
    InDoubleQuote,
}

#[derive(Debug, PartialEq)]
enum ParseError {
    UnclosedSingleQuote,
    UnclosedDoubleQuote,
}

struct Redirection {
    fd: u8,
    append: bool,
    target: String,
}

pub struct JobTable {
    pub jobs: Vec<JobEntry>,
    pub free_list: BTreeSet<usize>,
    pub next_id: usize,
    pub current_job_id: Option<usize>,
    pub prev_job_id: Option<usize>,
}

pub struct JobEntry {
    job_id: usize,
    pid: u32,
    status: JobStatus,
    cmd: String,
}

enum JobStatus {
    Running,
    Stopped,
    Done,
}

impl JobTable {
    fn allocate_job_id(&mut self) -> usize {
        if let Some(next_id) = self.free_list.pop_first() {
            next_id
        } else {
            let old_id = self.next_id;
            self.next_id += 1;
            old_id
        }
    }

    fn free_job_id(&mut self, id: usize) {
        self.free_list.insert(id);
    }

    fn job_marker(&self, job_id: usize) -> &'static str {
        if Some(job_id) == self.current_job_id {
            "+"
        } else if Some(job_id) == self.prev_job_id {
            "-"
        } else {
            " "
        }
    }

    fn remove_done_jobs(&mut self) {
        let done_indices: Vec<usize> = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| matches!(j.status, JobStatus::Done))
            .map(|(i, _)| i)
            .collect();
        let mut current_removed = false;
        for i in done_indices.iter().rev() {
            let job = self.jobs.remove(*i);
            if Some(job.job_id) == self.current_job_id {
                current_removed = true;
            }
            self.free_job_id(job.job_id);
        }
        if current_removed {
            self.current_job_id = self.prev_job_id;
        }
        let current = self.current_job_id;
        self.prev_job_id = self
            .jobs
            .iter()
            .filter(|j| Some(j.job_id) != current)
            .map(|j| j.job_id)
            .max();
    }
}

pub fn process_command(input: &str) {
    let result = process_input(input);
    match result {
        Ok(full_cmd) => handle_command(full_cmd),
        Err(ParseError::UnclosedSingleQuote) => println!("syntax error: unclosed single quote"),
        Err(ParseError::UnclosedDoubleQuote) => println!("syntax error: unclosed double quote"),
    }
}

fn handle_command(segments: Vec<Vec<String>>) {
    if segments.len() > 1 {
        handle_pipelines(segments);
    } else {
        handle_single_cmd(&segments[0]);
    }
}

fn handle_single_cmd(input: &Vec<String>) {
    let is_background = input.last().map(|s| s == "&").unwrap_or(false);
    let input = if is_background {
        &input[..input.len() - 1]
    } else {
        &input[..]
    };
    let cmd = &input[0];
    let all_args: Vec<String> = input.get(1..).unwrap_or_default().to_vec();
    let (args, redirections) = parse_args(all_args);
    match resolve_command(cmd) {
        CommandKind::Builtin => {
            let mut stdout: Box<dyn Write> = match redirections.iter().find(|r| r.fd == 1) {
                Some(r) => Box::new(open_redirect_file(r)),
                None => Box::new(io::stdout()),
            };
            let mut stderr: Box<dyn Write> = match redirections.iter().find(|r| r.fd == 2) {
                Some(r) => Box::new(open_redirect_file(r)),
                None => Box::new(io::stderr()),
            };
            run_builtin_command(cmd, &args, &mut *stdout, &mut *stderr);
        }
        CommandKind::External(path) => {
            run_command(path, &args, redirections, is_background, input.join(" "))
        }
        CommandKind::NotFound => println!("{}: not found", cmd),
    }
}

fn handle_pipelines(mut segments: Vec<Vec<String>>) {
    let is_background = segments
        .last()
        .and_then(|inner| inner.last())
        .is_some_and(|s| s == "&");
    if segments.iter().any(|s| s.is_empty()) {
        eprintln!("syntax error: empty command in pipeline");
        return;
    }
    if is_background {
        if let Some(inner) = segments.last_mut() {
            inner.pop();
        }
    }
    let mut pipes: Vec<(Option<OwnedFd>, Option<OwnedFd>)> = Vec::new();
    let mut children: Vec<process::Child> = Vec::new();
    for _ in 0..segments.len() - 1 {
        let (r, w) = nix::unistd::pipe().unwrap();
        pipes.push((Some(r), Some(w)));
    }
    for (i, segment) in segments.iter().enumerate() {
        let cmd = &segment[0];
        let all_args: Vec<String> = segment.get(1..).unwrap_or_default().to_vec();
        let (args, redirections) = parse_args(all_args);
        match resolve_command(cmd) {
            CommandKind::Builtin => {
                if i > 0 {
                    let _ = pipes[i - 1].0.take();
                }
                let mut buf: Vec<u8> = Vec::new();
                run_builtin_command(cmd, &args, &mut buf, &mut io::stderr());

                if i < segments.len() - 1 {
                    if let Some(write_fd) = pipes[i].1.take() {
                        let mut f = std::fs::File::from(write_fd);
                        let _ = f.write_all(&buf);
                    }
                } else {
                    let _ = io::stdout().write_all(&buf);
                }
            }
            CommandKind::External(path) => {
                let stdin = if i == 0 {
                    None
                } else {
                    pipes[i - 1].0.take().map(process::Stdio::from)
                };
                let stdout = if i == segments.len() - 1 {
                    None
                } else {
                    pipes[i].1.take().map(process::Stdio::from)
                };
                if let Some(child) = spawn_command(path, &args, &redirections, stdin, stdout) {
                    children.push(child);
                };
            }
            CommandKind::NotFound => println!("{}: not found", cmd),
        }
    }

    drop(pipes);
    if is_background {
        if let Some(last_child) = children.last_mut() {
            let mut job_table = get_job_table().lock().unwrap();
            let job_id = job_table.allocate_job_id();
            let pid = last_child.id();
            let full_cmd = segments
                .iter()
                .map(|s| s.join(" "))
                .collect::<Vec<_>>()
                .join(" | ");
            job_table.jobs.push(JobEntry {
                job_id,
                pid,
                status: JobStatus::Running,
                cmd: full_cmd,
            });
            job_table.prev_job_id = job_table.current_job_id;
            job_table.current_job_id = Some(job_id);
            println!("[{}] {}", job_id, pid);
        }
    } else {
        for mut child in children {
            let _ = child.wait();
        }
    }
}

fn expand_var<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) -> String {
    match chars.peek() {
        Some(&'{') => {
            chars.next();
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            get_variables()
                .lock()
                .unwrap()
                .get(&name)
                .cloned()
                .unwrap_or_default()
        }
        Some(&c) if c.is_ascii_alphanumeric() || c == '_' => {
            let mut name = String::new();
            while let Some(&c) = chars.peek() {
                if !c.is_ascii_alphanumeric() && c != '_' {
                    break;
                }
                name.push(c);
                chars.next();
            }
            get_variables()
                .lock()
                .unwrap()
                .get(&name)
                .cloned()
                .unwrap_or_default()
        }
        _ => String::from("$"),
    }
}

fn process_input(input: &str) -> Result<Vec<Vec<String>>, ParseError> {
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut args = Vec::new();
    let mut curr_token = String::new();
    let mut state = State::Default;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match state {
            State::Default => match c {
                ' ' => {
                    if !curr_token.is_empty() {
                        args.push(std::mem::take(&mut curr_token));
                    }
                }
                '\'' => state = State::InSingleQuote,
                '"' => state = State::InDoubleQuote,
                '\\' => match chars.next() {
                    Some(c) => curr_token.push(c),
                    None => continue,
                },
                '|' => {
                    if !curr_token.is_empty() {
                        args.push(std::mem::take(&mut curr_token));
                    }
                    segments.push(args);
                    args = Vec::new();
                }
                '$' => curr_token.push_str(&expand_var(&mut chars)),
                _ => curr_token.push(c),
            },
            State::InSingleQuote => match c {
                '\'' => state = State::Default,
                _ => curr_token.push(c),
            },
            State::InDoubleQuote => match c {
                '"' => state = State::Default,
                '\\' => match chars.peek() {
                    Some(c) => match c {
                        '"' | '\\' | '$' | '`' | '\n' => {
                            curr_token.push(*c);
                            chars.next();
                        }
                        _ => curr_token.push('\\'),
                    },
                    None => continue,
                },
                '$' => curr_token.push_str(&expand_var(&mut chars)),
                _ => curr_token.push(c),
            },
        }
    }
    if !curr_token.is_empty() {
        args.push(curr_token);
    }
    match state {
        State::InSingleQuote => Err(ParseError::UnclosedSingleQuote),
        State::InDoubleQuote => Err(ParseError::UnclosedDoubleQuote),
        State::Default => {
            segments.push(args);
            Ok(segments)
        }
    }
}

fn resolve_command(cmd: &str) -> CommandKind {
    if is_builtin(cmd) {
        CommandKind::Builtin
    } else {
        match find_command(cmd) {
            Some(path) => CommandKind::External(path),
            None => CommandKind::NotFound,
        }
    }
}

fn is_builtin(cmd: &str) -> bool {
    BUILTINS.contains(&cmd)
}

fn run_type(cmd: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) {
    match resolve_command(cmd) {
        CommandKind::Builtin => writeln!(stdout, "{} is a shell builtin", cmd).unwrap(),
        CommandKind::External(path) => writeln!(stdout, "{} is {}", cmd, path.display()).unwrap(),
        CommandKind::NotFound => writeln!(stderr, "{}: not found", cmd).unwrap(),
    }
}

fn find_command(cmd: &str) -> Option<PathBuf> {
    let full_path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&full_path)
        .flat_map(|dir| fs::read_dir(dir).ok().into_iter().flatten())
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
        .find(|e| e.path().file_name().map_or(false, |n| n == cmd))
        .map(|e| e.path())
}

fn spawn_command(
    cmd: PathBuf,
    args: &[String],
    redirections: &[Redirection],
    stdin_override: Option<process::Stdio>,
    stdout_override: Option<process::Stdio>,
) -> Option<process::Child> {
    let cmd_name = cmd.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stdin_stdio = stdin_override.unwrap_or(process::Stdio::inherit());
    let stdout_stdio =
        stdout_override.unwrap_or_else(|| match redirections.iter().find(|r| r.fd == 1) {
            Some(r) => process::Stdio::from(open_redirect_file(r)),
            None => process::Stdio::inherit(),
        });
    let stderr_stdio = match redirections.iter().find(|r| r.fd == 2) {
        Some(r) => process::Stdio::from(open_redirect_file(r)),
        None => process::Stdio::inherit(),
    };
    match process::Command::new(&cmd)
        .arg0(cmd_name)
        .args(args)
        .stdin(stdin_stdio)
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        .spawn()
    {
        Err(e) => {
            eprintln!("Failed to execute command: {}", e);
            None
        }
        Ok(child) => Some(child),
    }
}

fn run_command(
    cmd: PathBuf,
    args: &[String],
    redirections: Vec<Redirection>,
    is_background: bool,
    full_cmd: String,
) {
    if let Some(mut child) = spawn_command(cmd, args, &redirections, None, None) {
        if is_background {
            let mut job_table = get_job_table().lock().unwrap();
            let job_id = job_table.allocate_job_id();
            let pid = child.id();
            job_table.jobs.push(JobEntry {
                job_id,
                pid,
                status: JobStatus::Running,
                cmd: full_cmd,
            });
            job_table.prev_job_id = job_table.current_job_id;
            job_table.current_job_id = Some(job_id);
            println!("[{}] {}", job_id, pid);
        } else {
            let _ = child.wait();
        }
    }
}

fn run_builtin_command(cmd: &str, args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) {
    match cmd {
        "exit" => exit(0),
        "echo" => writeln!(stdout, "{}", args.join(" ")).unwrap(),
        "pwd" => match env::current_dir() {
            Ok(dir) => writeln!(stdout, "{}", dir.display()).unwrap(),
            Err(_) => writeln!(stderr, "pwd: error retrieving directory").unwrap(),
        },
        "complete" => handle_complete(&args, stdout, stderr),
        "cd" => {
            let path = args.first().map(|s| s.as_str()).unwrap_or("");
            let target = if path.is_empty() || path == "~" {
                env::var("HOME").unwrap_or_default()
            } else {
                path.to_string()
            };
            if let Err(_) = env::set_current_dir(&target) {
                writeln!(stderr, "cd: {}: No such file or directory", target).unwrap()
            }
        }
        "type" => run_type(
            args.first().map(|s| s.as_str()).unwrap_or(""),
            stdout,
            stderr,
        ),
        "jobs" => {
            reap_children(false);
            list_jobs(stdout)
        }
        "declare" => handle_declare(args, stdout, stderr),
        _ => writeln!(stderr, "{}: not found", cmd).unwrap(),
    }
}

fn parse_args(all_args: Vec<String>) -> (Vec<String>, Vec<Redirection>) {
    let mut redirections = Vec::new();
    let mut args = Vec::new();
    let mut i = 0;
    while i < all_args.len() {
        let token = all_args[i].as_str();
        match token {
            ">" | "1>" | "2>" | ">>" | "1>>" | "2>>" => {
                if i + 1 >= all_args.len() {
                    eprintln!("syntax error: expected file after redirection");
                    return (args, redirections);
                }
                let fd = if token.starts_with('2') { 2 } else { 1 };
                let append = token.ends_with(">>");
                let target = all_args[i + 1].clone();
                redirections.push(Redirection { fd, append, target });
                i += 2;
            }
            _ => {
                args.push(all_args[i].clone());
                i += 1;
            }
        }
    }
    (args, redirections)
}

fn open_redirect_file(r: &Redirection) -> std::fs::File {
    let path = std::path::Path::new(&r.target);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .append(r.append)
        .truncate(!r.append)
        .open(&r.target)
        .unwrap()
}

fn handle_complete(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) {
    if args.is_empty() {
        writeln!(stderr, "complete: usage: complete -C <script> <command>").unwrap();
        return;
    }
    match args[0].as_str() {
        "-C" => {
            if args.len() < 3 {
                writeln!(stderr, "complete: usage: complete -C <script> <command>").unwrap();
                return;
            }
            let script = args[1].clone();
            let cmd = args[2].clone();
            get_completions().lock().unwrap().insert(cmd, script);
        }
        "-p" => {
            if args.len() < 2 {
                writeln!(stderr, "complete: usage: complete -p <command>").unwrap();
                return;
            }
            let cmd = args[1].clone();
            if let Some(script) = get_completions().lock().unwrap().get(&cmd) {
                writeln!(stdout, "complete -C '{}' {}", script, cmd).unwrap();
                return;
            }
            writeln!(stderr, "complete: {}: no completion specification", cmd).unwrap();
        }
        "-r" => {
            if args.len() < 2 {
                writeln!(stderr, "complete: usage: complete -r <command>").unwrap();
                return;
            }
            let cmd = args[1].clone();
            get_completions().lock().unwrap().remove_entry(&cmd);
        }
        _ => {
            writeln!(stderr, "complete: unsupported option: {}", args[0]).unwrap();
        }
    }
}

fn list_jobs(stdout: &mut dyn Write) {
    let mut job_table = get_job_table().lock().unwrap();

    for job in job_table.jobs.iter() {
        let marker = job_table.job_marker(job.job_id);
        let (status, show_ampersand) = match job.status {
            JobStatus::Running => (format!("{:<24}", "Running"), true),
            JobStatus::Done => (format!("{:<24}", "Done"), false),
            JobStatus::Stopped => (format!("{:<24}", "Stopped"), true),
        };
        let cmd = if show_ampersand {
            format!("{} &", job.cmd)
        } else {
            job.cmd.clone()
        };
        let out = if marker == " " {
            format!("[{}]   {}{}", job.job_id, status, cmd)
        } else {
            format!("[{}]{}  {}{}", job.job_id, marker, status, cmd)
        };
        writeln!(stdout, "{}", out).unwrap();
    }

    job_table.remove_done_jobs();
}

fn handle_declare(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) {
    let mut flags = HashSet::new();
    let mut operand = None;
    for arg in args {
        if arg.starts_with('-') && operand.is_none() {
            for ch in arg[1..].chars() {
                flags.insert(ch);
            }
        } else {
            operand = Some(arg.as_str());
            break;
        }
    }
    match operand {
        Some(op) => {
            if flags.is_empty() {
                let declartion: Vec<&str> = op.splitn(2, "=").collect();
                if declartion.len() < 2 {
                    writeln!(stderr, "declare: no value or identifier present").unwrap();
                    return;
                }
                if !validate_var_name(declartion[0]) {
                    writeln!(stderr, "declare: `{}': not a valid identifier", op).unwrap();
                    return;
                }
                get_variables()
                    .lock()
                    .unwrap()
                    .insert(String::from(declartion[0]), String::from(declartion[1]));
            } else if flags.contains(&'p') {
                if let Some(value) = get_variables().lock().unwrap().get(op) {
                    writeln!(stdout, "declare -- {}=\"{}\"", op, value).unwrap();
                } else {
                    writeln!(stderr, "declare: {}: not found", op).unwrap();
                }
            }
        }
        None => {}
    }
}

fn validate_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

pub fn reap_children(notify: bool) {
    let mut job_table = get_job_table().lock().unwrap();
    for job in job_table.jobs.iter_mut() {
        match waitpid(Pid::from_raw(job.pid as i32), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, _)) => job.status = JobStatus::Done,
            Ok(WaitStatus::Signaled(_, _, _)) => job.status = JobStatus::Done,
            Ok(WaitStatus::Stopped(_, _)) => job.status = JobStatus::Stopped,
            _ => {}
        }
    }
    if notify {
        for job in job_table
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Done))
        {
            let marker = job_table.job_marker(job.job_id);
            println!("[{}]{}  {:<24}{}", job.job_id, marker, "Done", job.cmd);
        }
        job_table.remove_done_jobs();
    }
}
