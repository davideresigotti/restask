//! Rules the vault cannot express (§11.6): an `RRULE` that is not managed stays among a
//! resource's extras. These helpers find it there and hand it back with one occurrence
//! consumed. Pure — no I/O.

use crate::domain::Recurrence;

/// Finds the task's own `RRULE` among `extras` (lines nested in a sub-component such as
/// `VALARM` are not the task's): its index and the rule as far as it is understood.
pub fn find_in_extras(extras: &[String]) -> Option<(usize, Recurrence)> {
    let mut depth = 0usize;
    for (index, line) in extras.iter().enumerate() {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
        } else if upper.starts_with("END:") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && is_rrule(&upper) {
            let value = line.split_once(':')?.1;
            return Recurrence::from_rrule(value).map(|(rule, _)| (index, rule));
        }
    }
    None
}

/// `true` for an (upper-cased) `RRULE` content line.
pub fn is_rrule(upper_line: &str) -> bool {
    upper_line.starts_with("RRULE:") || upper_line.starts_with("RRULE;")
}

/// The `RRULE` line to store after one occurrence was completed: exactly as written,
/// except that a `COUNT` is decremented.
pub fn consume_count(line: &str) -> String {
    let Some((head, value)) = line.split_once(':') else {
        return line.to_string();
    };
    let parts: Vec<String> = value
        .split(';')
        .map(|part| match part.split_once('=') {
            Some((key, count)) if key.trim().eq_ignore_ascii_case("COUNT") => {
                match count.trim().parse::<u32>() {
                    Ok(n) => format!("{key}={}", n.saturating_sub(1)),
                    Err(_) => part.to_string(),
                }
            }
            _ => part.to_string(),
        })
        .collect();
    format!("{head}:{}", parts.join(";"))
}
