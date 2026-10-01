//! TTY primitives for the setup wizard (§13.2). Thin by design — reading a line, hiding
//! a password; the wizard's logic lives in [`crate::setup`].

use std::io::{self, BufRead, Write};

use crate::RestaskError;

/// Prints `message` and reads one trimmed line from stdin.
pub fn prompt(message: &str) -> Result<String, RestaskError> {
    print!("{message} ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Prints `message` and reads one hidden line (rpassword, §13.2 step 4).
pub fn secret(message: &str) -> Result<String, RestaskError> {
    Ok(rpassword::prompt_password(message)?.trim().to_string())
}

/// Yes/no confirmation; empty input means no.
pub fn confirm(message: &str) -> Result<bool, RestaskError> {
    loop {
        let answer = prompt(&format!("{message} [y/N]"))?;
        match answer.as_str() {
            "y" | "Y" | "yes" | "Yes" => return Ok(true),
            "" | "n" | "N" | "no" | "No" => return Ok(false),
            _ => {}
        }
    }
}
