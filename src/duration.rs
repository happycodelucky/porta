use std::collections::HashSet;
use std::time::Duration;

use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::error::{PortaError, Result};

const SECOND: u64 = 1;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;

pub fn parse_duration(input: &str, bare_minutes: bool, allow_zero: bool) -> Result<Duration> {
    if input.is_empty() {
        return Err(invalid_duration("duration cannot be empty"));
    }

    if input.bytes().all(|byte| byte.is_ascii_digit()) {
        if !bare_minutes {
            return Err(invalid_duration("duration requires a unit"));
        }
        let minutes = parse_number(input)?;
        return checked_duration(minutes, MINUTE, allow_zero);
    }

    let bytes = input.as_bytes();
    let mut index = 0;
    let mut seconds = 0_u64;
    let mut seen = HashSet::new();

    while index < bytes.len() {
        let number_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if number_start == index || index == bytes.len() {
            return Err(invalid_duration(format!("invalid duration: {input}")));
        }

        let number = parse_number(&input[number_start..index])?;
        let unit = char::from(bytes[index]);
        index += 1;
        if !seen.insert(unit) {
            return Err(invalid_duration(format!(
                "duration unit '{unit}' appears more than once"
            )));
        }
        let multiplier = match unit {
            's' => SECOND,
            'm' => MINUTE,
            'h' => HOUR,
            'd' => DAY,
            'w' => WEEK,
            _ => {
                return Err(invalid_duration(format!(
                    "unsupported duration unit: {unit}"
                )));
            }
        };
        let component = number
            .checked_mul(multiplier)
            .ok_or_else(|| invalid_duration("duration is too large"))?;
        seconds = seconds
            .checked_add(component)
            .ok_or_else(|| invalid_duration("duration is too large"))?;
    }

    if seconds == 0 && !allow_zero {
        return Err(invalid_duration("duration must be positive"));
    }
    Ok(Duration::from_secs(seconds))
}

#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let mut seconds = duration.as_secs();
    if seconds == 0 {
        return "0s".to_owned();
    }

    let mut output = String::new();
    for (unit, size) in [
        ('w', WEEK),
        ('d', DAY),
        ('h', HOUR),
        ('m', MINUTE),
        ('s', SECOND),
    ] {
        let count = seconds / size;
        if count > 0 {
            output.push_str(&count.to_string());
            output.push(unit);
            seconds %= size;
        }
    }
    output
}

pub fn parse_expiration(input: &str, now: OffsetDateTime) -> Result<OffsetDateTime> {
    if let Ok(timestamp) = OffsetDateTime::parse(input, &Rfc3339) {
        if timestamp <= now {
            return Err(invalid_duration("expiration must be in the future"));
        }
        return Ok(timestamp.to_offset(UtcOffset::UTC));
    }

    let duration = parse_duration(input, false, false)?;
    let duration = time::Duration::try_from(duration)
        .map_err(|_| invalid_duration("expiration duration is too large"))?;
    now.checked_add(duration)
        .ok_or_else(|| invalid_duration("expiration is too large"))
}

pub fn format_timestamp(value: OffsetDateTime) -> Result<String> {
    value
        .format(&Rfc3339)
        .map_err(|error| PortaError::infrastructure("timestamp_failed", error.to_string()))
}

fn parse_number(value: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| invalid_duration("duration component is too large"))
}

fn checked_duration(value: u64, multiplier: u64, allow_zero: bool) -> Result<Duration> {
    let seconds = value
        .checked_mul(multiplier)
        .ok_or_else(|| invalid_duration("duration is too large"))?;
    if seconds == 0 && !allow_zero {
        return Err(invalid_duration("duration must be positive"));
    }
    Ok(Duration::from_secs(seconds))
}

fn invalid_duration(message: impl Into<String>) -> PortaError {
    PortaError::invalid("invalid_duration", message)
}

#[cfg(test)]
mod tests {
    use time::{OffsetDateTime, UtcOffset};

    use super::{format_duration, format_timestamp, parse_duration, parse_expiration};

    #[test]
    fn parses_combined_duration() {
        let value = parse_duration("2d3h5m", false, false).expect("duration should parse");
        assert_eq!(value.as_secs(), 183_900);
        assert_eq!(format_duration(value), "2d3h5m");
    }

    #[test]
    fn bare_timeout_uses_minutes() {
        let value = parse_duration("24", true, false).expect("duration should parse");
        assert_eq!(value.as_secs(), 1_440);
    }

    #[test]
    fn zero_requires_explicit_permission() {
        assert!(parse_duration("0s", false, false).is_err());
        assert_eq!(
            parse_duration("0s", false, true)
                .expect("zero should parse")
                .as_secs(),
            0
        );
    }

    #[test]
    fn absolute_expiration_is_normalized_to_utc() {
        let now = OffsetDateTime::from_unix_timestamp(0).expect("Unix epoch");
        let expiration =
            parse_expiration("2026-08-01T12:00:00+05:00", now).expect("expiration should parse");
        assert_eq!(expiration.offset(), UtcOffset::UTC);
        assert_eq!(
            format_timestamp(expiration).expect("timestamp should format"),
            "2026-08-01T07:00:00Z"
        );
    }
}
