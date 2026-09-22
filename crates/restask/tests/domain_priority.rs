//! Integration tests for `domain::priority` (§3.2): emoji ⇄ enum ⇄ ical, both directions.

use restask::domain::Priority;

#[test]
fn all_is_highest_to_lowest() {
    assert_eq!(
        Priority::ALL,
        [
            Priority::Highest,
            Priority::High,
            Priority::Medium,
            Priority::Low,
            Priority::Lowest
        ]
    );
}

#[test]
fn emoji_table_exact() {
    assert_eq!(Priority::Highest.emoji(), "🔺");
    assert_eq!(Priority::High.emoji(), "⏫");
    assert_eq!(Priority::Medium.emoji(), "🔼");
    assert_eq!(Priority::Low.emoji(), "🔽");
    assert_eq!(Priority::Lowest.emoji(), "⏬");
}

#[test]
fn emoji_codepoints_exact() {
    let pairs = [
        (Priority::Highest, [0x1F53A]),
        (Priority::High, [0x23EB]),
        (Priority::Medium, [0x1F53C]),
        (Priority::Low, [0x1F53D]),
        (Priority::Lowest, [0x23EC]),
    ];
    for (priority, codepoints) in pairs {
        let chars: Vec<char> = priority.emoji().chars().collect();
        let expected: Vec<char> = codepoints
            .iter()
            .map(|&c| char::from_u32(c).unwrap())
            .collect();
        assert_eq!(chars, expected, "codepoint drift for {priority:?}");
    }
}

#[test]
fn from_emoji_round_trip_both_directions() {
    for priority in Priority::ALL {
        assert_eq!(Priority::from_emoji(priority.emoji()), Some(priority));
    }
    assert_eq!(Priority::from_emoji("▶️"), None);
    assert_eq!(Priority::from_emoji("🔺x"), None);
    assert_eq!(Priority::from_emoji(""), None);
}

#[test]
fn to_ical_table() {
    assert_eq!(Priority::Highest.to_ical(), 1);
    assert_eq!(Priority::High.to_ical(), 3);
    assert_eq!(Priority::Medium.to_ical(), 5);
    assert_eq!(Priority::Low.to_ical(), 7);
    assert_eq!(Priority::Lowest.to_ical(), 9);
}

#[test]
fn from_ical_all_eleven_inputs() {
    assert_eq!(Priority::from_ical(0), None);
    assert_eq!(Priority::from_ical(1), Some(Priority::Highest));
    assert_eq!(Priority::from_ical(2), Some(Priority::Highest));
    assert_eq!(Priority::from_ical(3), Some(Priority::High));
    assert_eq!(Priority::from_ical(4), Some(Priority::High));
    assert_eq!(Priority::from_ical(5), Some(Priority::Medium));
    assert_eq!(Priority::from_ical(6), Some(Priority::Medium));
    assert_eq!(Priority::from_ical(7), Some(Priority::Low));
    assert_eq!(Priority::from_ical(8), Some(Priority::Low));
    assert_eq!(Priority::from_ical(9), Some(Priority::Lowest));
    assert_eq!(Priority::from_ical(10), None);
}

#[test]
fn ical_round_trip_both_directions() {
    for priority in Priority::ALL {
        assert_eq!(Priority::from_ical(priority.to_ical()), Some(priority));
    }
}

#[test]
fn heading_table() {
    assert_eq!(Priority::Highest.heading(), "Highest");
    assert_eq!(Priority::High.heading(), "High");
    assert_eq!(Priority::Medium.heading(), "Medium");
    assert_eq!(Priority::Low.heading(), "Low");
    assert_eq!(Priority::Lowest.heading(), "Lowest");
}

#[test]
fn cli_name_round_trip() {
    for priority in Priority::ALL {
        assert_eq!(Priority::from_cli_name(priority.cli_name()), Some(priority));
    }
    assert_eq!(Priority::from_cli_name("HIGH"), Some(Priority::High));
    assert_eq!(Priority::from_cli_name("urgent"), None);
    assert_eq!(Priority::from_cli_name(""), None);
}

#[test]
fn serde_round_trip() {
    for priority in Priority::ALL {
        let json = serde_json::to_string(&priority).unwrap();
        let back: Priority = serde_json::from_str(&json).unwrap();
        assert_eq!(back, priority);
    }
}
