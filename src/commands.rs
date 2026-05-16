use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process;
use std::process::exit;

const BUILTINS: &[&str] = &["exit", "echo", "pwd", "type", "cd", "cat"];

enum CommandKind {
    Builtin,
    External(PathBuf),
    NotFound,
}

#[derive(Debug, PartialEq)]
enum State {
    Default,
    InSingleQuote,
}

#[derive(Debug, PartialEq)]
enum ParseError {
    UnclosedSingleQuote,
}

pub fn process_command(input: &str) {
    let result = process_input(input);
    match result {
        Ok(full_cmd) => handle_command(full_cmd),
        Err(ParseError::UnclosedSingleQuote) => {
            println!("syntax error: unclosed single quote")
        }
    }
}

fn handle_command(input: Vec<String>) {
    let cmd = &input[0];
    let args: String = input.get(1..).map(|s| s.join(" ")).unwrap_or_default();
    match resolve_command(&cmd) {
        CommandKind::Builtin => run_builtin_command(cmd, args),
        CommandKind::External(path) => run_command(path, &args),
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
                _ => curr_token.push(c),
            },
            State::InSingleQuote => match c {
                '\'' => state = State::Default,
                _ => curr_token.push(c),
            },
        }
    }
    if !curr_token.is_empty() {
        args.push(curr_token);
    }
    match state {
        State::InSingleQuote => Err(ParseError::UnclosedSingleQuote),
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

fn run_type(cmd: &str) {
    match resolve_command(cmd) {
        CommandKind::Builtin => println!("{} is a shell builtin", cmd),
        CommandKind::External(path) => println!("{} is {}", cmd, path.display()),
        CommandKind::NotFound => println!("{}: not found", cmd),
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

fn run_command(cmd: PathBuf, args: &str) {
    let cmd_name = cmd.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let output = process::Command::new(&cmd)
        .arg0(cmd_name)
        .args(args.split_whitespace())
        .output();
    match output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                print!("{}", stderr);
                return;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            print!("{}", stdout);
        }
        Err(_) => print!("Failed to execute command"),
    }
}

fn run_builtin_command(cmd: &String, args: String) {
    match cmd.as_str() {
        "exit" => exit(0),
        "echo" => println!("{}", args),
        "pwd" => match env::current_dir() {
            Ok(dir) => println!("{}", dir.display()),
            Err(_) => println!("{}: not found", cmd),
        },
        "cd" => {
            let path = args.split_whitespace().next().unwrap_or_default();
            if path.is_empty() || path == "~" {
                if let Err(_) = env::set_current_dir(env::var("HOME").unwrap()) {
                    println!("cd: {}: No such file or directory", path);
                }
            } else if let Err(_) = env::set_current_dir(path) {
                println!("cd: {}: No such file or directory", path);
            }
        }
        "cat" => {
            if let Some(path) = find_command("cat") {
                run_command(path, &args);
                println!()
            }
        }
        "type" => run_type(&args),
        _ => println!("{}: not found", cmd),
    }
}
