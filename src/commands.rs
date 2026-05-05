use std::fs;
use std::process::exit;
use std::os::unix::fs::PermissionsExt;
use std::path::{PathBuf};
use std::process;
use std::os::unix::process::CommandExt;

pub fn process_command(input: &str) {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts.get(0).copied().unwrap_or("");
    let args: String = parts.get(1..).map(|s| s.join(" ")).unwrap_or_default();
    match cmd {
        "exit" => exit(0),
        "echo" => println!("{}", args),
        "type" => process_type(&args),
        _ => find_command(cmd).map(|path| run_command(path, &args)).unwrap_or_else(|| println!("{}: not found", cmd))
    }
}

fn process_type(args: &str) {
    let cmd = args.split_whitespace().next().unwrap_or("");
    match cmd {
        "exit" | "echo" | "type" => println!("{} is a shell builtin", args.trim()),
        _ => match find_command(cmd) {
            Some(path ) => run_command(path, args),
            None => println!("{}: not found", cmd)
        }
    }
}

fn find_command(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| fs::read_dir(dir).ok().into_iter().flatten())
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
        })
        .find(|e| e.path().file_name().map_or(false, |n| n == cmd))
        .map(|e| e.path())
}

fn run_command(cmd: PathBuf, args: &str) {
    let cmd_name = cmd.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let output = process::Command::new(&cmd).arg0(cmd_name).args(args.split_whitespace()).output();
    match output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                print!("{}", stderr);
                return;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            print!("{}", stdout);
        },
        Err(e) => print!("Failed to execute command")
    }
}