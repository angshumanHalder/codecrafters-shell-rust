use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process;
use std::process::exit;

const BUILTINS: &[&str] = &["exit", "echo", "pwd", "type", "cd"];

enum CommandKind {
    Builtin,
    External(PathBuf),
    NotFound,
}

pub fn process_command(input: &str) {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts.get(0).copied().unwrap_or("");
    let args: String = parts.get(1..).map(|s| s.join(" ")).unwrap_or_default();
    match resolve_command(cmd) {
        CommandKind::Builtin => run_builtin_command(cmd, args),
        CommandKind::External(path) => run_command(path, &args),
        CommandKind::NotFound => println!("{}: not found", cmd),
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

fn run_builtin_command(cmd: &str, args: String) {
    match cmd {
        "exit" => exit(0),
        "echo" => println!("{}", args),
        "pwd" => match env::current_dir() {
            Ok(dir) => println!("{}", dir.display()),
            Err(_) => println!("{}: not found", cmd),
        },
        "cd" => {
            let path = args
                .split_whitespace()
                .into_iter()
                .next()
                .unwrap_or_default();
            // if path == "" || path == "~" {
            //     if let Err(_) = env::set_current_dir(env::home_dir().unwrap()) {
            //         println!("cd: {}: No such file or directory", path);
            //     }
            // }

            // if path.starts_with("/") {
            //     if let Err(_) = env::set_current_dir(path) {
            //         println!("cd: {}: No such file or directory", path);
            //     }
            // }

            // if path.starts_with("./") || path.starts_with("../") {
            let normalized_path = Path::new(path).canonicalize().unwrap();
            if let Err(_) = env::set_current_dir(normalized_path) {
                println!("cd: {}: No such file or directory", path);
            }
            // }
        }
        "type" => run_type(&args),
        _ => println!("{}: not found", cmd),
    }
}
