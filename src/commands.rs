use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
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

pub const BUILTINS: &[&str] = &["exit", "echo", "pwd", "type", "cd", "complete", "jobs"];

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
}

pub fn process_command(input: &str) {
    let result = process_input(input);
    match result {
        Ok(full_cmd) => handle_command(full_cmd),
        Err(ParseError::UnclosedSingleQuote) => println!("syntax error: unclosed single quote"),
        Err(ParseError::UnclosedDoubleQuote) => println!("syntax error: unclosed double quote"),
    }
}

fn handle_command(input: Vec<String>) {
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

fn process_input(input: &str) -> Result<Vec<String>, ParseError> {
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
        State::Default => Ok(args),
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

fn run_command(
    cmd: PathBuf,
    args: &[String],
    redirections: Vec<Redirection>,
    is_background: bool,
    full_cmd: String,
) {
    let cmd_name = cmd.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stdout_stdio = match redirections.iter().find(|r| r.fd == 1) {
        Some(r) => process::Stdio::from(open_redirect_file(r)),
        None => process::Stdio::inherit(),
    };
    let stderr_stdio = match redirections.iter().find(|r| r.fd == 2) {
        Some(r) => process::Stdio::from(open_redirect_file(r)),
        None => process::Stdio::inherit(),
    };
    let child = process::Command::new(&cmd)
        .arg0(cmd_name)
        .args(args)
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        .spawn();
    match child {
        Err(e) => eprintln!("Failed to execute command: {}", e),
        Ok(mut child) => {
            if is_background {
                let mut job_table = get_job_table().lock().unwrap();
                let job_id = job_table.allocate_job_id();
                let pid = child.id();
                let jobs = &mut job_table.jobs;
                jobs.push(JobEntry {
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
            reap_children();
            list_jobs(stdout)
        }
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
    let mut to_remove: Vec<usize> = Vec::new();
    let mut job_table = get_job_table().lock().unwrap();
    let current = job_table.current_job_id;
    let prev = job_table.prev_job_id;

    for (i, job) in job_table.jobs.iter().enumerate() {
        let job_num = job.job_id;
        let status = match job.status {
            JobStatus::Running => format!("{:<24}", "Running"),
            JobStatus::Done => {
                to_remove.push(i);
                format!("{:<24}", "Done")
            }
            JobStatus::Stopped => format!("{:<24}", "Stopped"),
        };
        let cmd = &job.cmd;
        let mut out = format!("[{}]   {}{}", job_num, status, cmd);
        if Some(job.job_id) == current {
            out = format!("[{}]+  {}{}", job_num, status, cmd);
        } else if Some(job.job_id) == prev {
            out = format!("[{}]-  {}{}", job_num, status, cmd);
        }
        writeln!(stdout, "{}", out).unwrap();
    }

    for i in to_remove.iter().rev() {
        let job = job_table.jobs.remove(*i);
        job_table.free_job_id(job.job_id);
    }
}

pub fn reap_children() {
    let mut job_table = get_job_table().lock().unwrap();
    for job in job_table.jobs.iter_mut() {
        match waitpid(Pid::from_raw(job.pid as i32), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_pid, _signal)) => {
                job.status = JobStatus::Done;
            }
            Ok(WaitStatus::Signaled(_pid, _signal, _)) => {
                job.status = JobStatus::Done;
            }
            Ok(WaitStatus::Stopped(_pid, _signal)) => {
                job.status = JobStatus::Stopped;
            }
            Ok(WaitStatus::StillAlive) => {}
            Err(_) => {}
            _ => {}
        }
    }
}
