use std::fs;
use std::process::exit;

pub fn process_command(input: &str) {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts.get(0).copied().unwrap_or("");
    let args: String = parts.get(1..).map(|s| s.join(" ")).unwrap_or_default();
    match cmd {
        "exit" => exit(0),
        "echo" => println!("{}", args),
        "type" => process_type(&args),
        _ => println!("{}: command not found", cmd),
    }
}

fn process_type(cmd: &str) {
    match cmd {
        "exit" | "echo" | "type" => println!("{} is a shell builtin", cmd),
        _ => match find_command(cmd) {
            Some(path ) => println!("{} is {}", cmd, path),
            None => println!("{}: not found", cmd)
        }
    }
}

fn find_command(cmd: &str) -> Option<String> {
    if let Some(path) = std::env::var_os("PATH") {
        let env_paths = std::env::split_paths(&path);
        for path in env_paths {
            let entries = fs::read_dir(path);
            match entries {
                Ok(entries) => {
                    for e in entries {
                        match e {
                            Ok(entry) => if let Some(file_name) = entry.path().file_name() {
                                if file_name == cmd {
                                    let full_path = entry.path().display().to_string();
                                    return Some(full_path);
                                }
                            },
                            Err(_) => continue,
                        }
                    }
                }
                Err(_) => continue
            }
        }
    }
    None
}