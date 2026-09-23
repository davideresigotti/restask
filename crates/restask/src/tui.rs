//! TTY primitives for the setup wizard (§13.2). Thin by design — reading a line, hiding
//! a password, picking from a list; the wizard's logic lives in [`crate::setup`].

use std::io::{self, BufRead, Write};

use crate::TaskresError;

/// Prints `message` and reads one trimmed line from stdin.
pub fn prompt(message: &str) -> Result<String, TaskresError> {
    print!("{message} ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Prints `message` and reads one hidden line (rpassword, §13.2 step 4).
pub fn secret(message: &str) -> Result<String, TaskresError> {
    Ok(rpassword::prompt_password(message)?.trim().to_string())
}

/// Prints numbered `options` and returns the chosen 0-based index; re-prompts on junk.
pub fn select(message: &str, options: &[&str]) -> Result<usize, TaskresError> {
    loop {
        println!("{message}");
        for (index, option) in options.iter().enumerate() {
            println!("  {}) {option}", index + 1);
        }
        let answer = prompt("choice:")?;
        if let Ok(choice) = answer.parse::<usize>() {
            if (1..=options.len()).contains(&choice) {
                return Ok(choice - 1);
            }
        }
    }
}

/// Yes/no confirmation; empty input means no.
pub fn confirm(message: &str) -> Result<bool, TaskresError> {
    loop {
        let answer = prompt(&format!("{message} [y/N]"))?;
        match answer.as_str() {
            "y" | "Y" | "yes" | "Yes" => return Ok(true),
            "" | "n" | "N" | "no" | "No" => return Ok(false),
            _ => {}
        }
    }
}
