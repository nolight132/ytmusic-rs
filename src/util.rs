use std::time::Duration;

pub fn parse_clock(text: &str) -> Option<Duration> {
    let mut seconds: u64 = 0;
    for part in text.split(':') {
        let value: u64 = part.trim().parse().ok()?;
        seconds = seconds * 60 + value;
    }
    Some(Duration::from_secs(seconds))
}

pub fn parse_year(text: &str) -> Option<i32> {
    let year: i32 = text.trim().parse().ok()?;
    (1000..=9999).contains(&year).then_some(year)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_minutes() {
        assert_eq!(parse_clock("3:05"), Some(Duration::from_secs(185)));
    }

    #[test]
    fn clock_hours() {
        assert_eq!(parse_clock("1:02:03"), Some(Duration::from_secs(3723)));
    }

    #[test]
    fn clock_invalid() {
        assert_eq!(parse_clock("abc"), None);
    }

    #[test]
    fn year_valid() {
        assert_eq!(parse_year("2021"), Some(2021));
        assert_eq!(parse_year("21"), None);
    }
}
