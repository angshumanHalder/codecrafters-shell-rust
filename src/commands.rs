use std::fs;
use std::process::exit;
use std::os::unix::fs::PermissionsExt;
use std::process;

pub fn process_command(input: &str) {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts.get(0).copied().unwrap_or("");
    let args: String = parts.get(1..).map(|s| s.join(" ")).unwrap_or_default();
    match cmd {
        "exit" => exit(0),
        "echo" => println!("{}", args),
        "type" => process_type(cmd, &args),
        _ => println!("{}: command not found", cmd),
    }
}

fn process_type(cmd: &str, args: &str) {
    match cmd {
        "exit" | "echo" | "type" => println!("{} is a shell builtin", cmd),
        _ => match find_command(cmd) {
            Some(path ) => run_command(&path, args),
            None => println!("{}: not found", cmd)
        }
    }
}

fn find_command(cmd: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| fs::read_dir(dir).ok().into_iter().flatten())
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
        })
        .find(|e| e.path().file_name().map_or(false, |n| n == cmd))
        .map(|e| e.path().display().to_string())
}

fn run_command(cmd: &str, args: &str) {
    let output = process::Command::new(cmd).args(args.split_whitespace()).output();
    match output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("{}", stderr);
                return;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("{}", stdout);
        },
        Err(e) => println!("Failed to execute command: {} \n {}", cmd, e)
    }
}