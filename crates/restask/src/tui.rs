//! TTY primitives for the setup wizard (§13.2). Thin by design — reading a line, hiding
//! a password, a list to pick from; the wizard's logic lives in [`crate::setup`].

use std::io::{self, BufRead, IsTerminal, Write};

use dialoguer::{MultiSelect, Select};

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

/// Yes/no confirmation for a step the wizard recommends; empty input means yes.
pub fn confirm_yes(message: &str) -> Result<bool, RestaskError> {
    loop {
        let answer = prompt(&format!("{message} [Y/n]"))?;
        match answer.as_str() {
            "" | "y" | "Y" | "yes" | "Yes" => return Ok(true),
            "n" | "N" | "no" | "No" => return Ok(false),
            _ => {}
        }
    }
}

/// `true` when the wizard talks to a person at a terminal, so a list can be moved through
/// with the keys ([`pick_many`], [`pick_one`]); with piped input the questions are typed
/// answers instead.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Shows `items` as a checklist, each ticked as `checked` says, and returns the indexes
/// left ticked when Enter is pressed: arrows move, the spacebar ticks and unticks one,
/// `a` all of them.
pub fn pick_many(
    message: &str,
    items: &[String],
    checked: &[bool],
) -> Result<Vec<usize>, RestaskError> {
    MultiSelect::new()
        .with_prompt(message)
        .items(items)
        .defaults(checked)
        .interact()
        .map_err(picker_error)
}

/// Shows `items` as a list and returns the index of the one Enter is pressed on.
pub fn pick_one(message: &str, items: &[String]) -> Result<usize, RestaskError> {
    Select::new()
        .with_prompt(message)
        .items(items)
        .default(0)
        .interact()
        .map_err(picker_error)
}

fn picker_error(error: dialoguer::Error) -> RestaskError {
    match error {
        dialoguer::Error::IO(error) => error.into(),
    }
}
