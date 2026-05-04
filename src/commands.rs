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
        "exit" | "echo" => println!("{} is a shell builtin", cmd),
        _ => println!("{}: command not found", cmd),
    }
}