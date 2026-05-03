#[allow(unused_imports)]
use std::io::{self, Write};

fn main() {
    // TODO: Uncomment the code below to pass the first stage
    let mut input = String::new();
    print!("$ ");
    io::stdin().read_line(&mut input).unwrap();
    println!("{}: command not found", input);
    io::stdout().flush().unwrap();
}
