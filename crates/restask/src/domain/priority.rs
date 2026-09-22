//! Task priority (§3.2): emoji ⇄ enum ⇄ iCalendar PRIORITY mapping.

/// Task priority with exact emoji codepoints from the §3.2 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Priority {
    /// 🔺 (U+1F53A), VTODO `PRIORITY` 1.
    Highest,
    /// ⏫ (U+23EB), VTODO `PRIORITY` 3.
    High,
    /// 🔼 (U+1F53C), VTODO `PRIORITY` 5.
    Medium,
    /// 🔽 (U+1F53D), VTODO `PRIORITY` 7.
    Low,
    /// ⏬ (U+23EC), VTODO `PRIORITY` 9.
    Lowest,
}

impl Priority {
    /// All priorities, highest first.
    pub const ALL: [Priority; 5] = [
        Priority::Highest,
        Priority::High,
        Priority::Medium,
        Priority::Low,
        Priority::Lowest,
    ];

    /// Exact emoji literal for this priority (§3.2: 🔺 ⏫ 🔼 🔽 ⏬).
    pub fn emoji(self) -> &'static str {
        match self {
            Priority::Highest => "🔺",
            Priority::High => "⏫",
            Priority::Medium => "🔼",
            Priority::Low => "🔽",
            Priority::Lowest => "⏬",
        }
    }

    /// Recognizes an emoji literal exactly as produced by [`Priority::emoji`].
    pub fn from_emoji(s: &str) -> Option<Self> {
        Priority::ALL.into_iter().find(|p| p.emoji() == s)
    }

    /// iCalendar `PRIORITY` value (1, 3, 5, 7, 9).
    pub fn to_ical(self) -> u8 {
        match self {
            Priority::Highest => 1,
            Priority::High => 3,
            Priority::Medium => 5,
            Priority::Low => 7,
            Priority::Lowest => 9,
        }
    }

    /// Reverse iCalendar mapping: `1,2→Highest 3,4→High 5,6→Medium 7,8→Low 9→Lowest`;
    /// everything else (including 0) → [`None`].
    pub fn from_ical(v: u8) -> Option<Self> {
        match v {
            1 | 2 => Some(Priority::Highest),
            3 | 4 => Some(Priority::High),
            5 | 6 => Some(Priority::Medium),
            7 | 8 => Some(Priority::Low),
            9 => Some(Priority::Lowest),
            _ => None,
        }
    }

    /// TODO.md section heading ("Highest" .. "Lowest").
    pub fn heading(self) -> &'static str {
        match self {
            Priority::Highest => "Highest",
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
            Priority::Lowest => "Lowest",
        }
    }

    /// Lowercase CLI name ("highest" .. "lowest").
    pub fn cli_name(self) -> &'static str {
        match self {
            Priority::Highest => "highest",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
            Priority::Lowest => "lowest",
        }
    }

    /// Recognizes a CLI name, case-insensitively.
    pub fn from_cli_name(s: &str) -> Option<Self> {
        let lowered = s.to_lowercase();
        Priority::ALL.into_iter().find(|p| p.cli_name() == lowered)
    }
}
