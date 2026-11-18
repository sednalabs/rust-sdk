//! Shared clock formatting without changing timestamp strings on the wire.

use std::fmt::Write as _;

use time::{OffsetDateTime, UtcOffset};

/// Keep UTC's explicit offset and the previous whole/milli/micro/nanosecond
/// precision. Inbound annotation strings are deliberately not parsed here.
pub(crate) fn rfc3339_utc(timestamp: OffsetDateTime) -> String {
    let timestamp = timestamp.to_offset(UtcOffset::UTC);
    let year = timestamp.year();
    let mut output = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        // Preserve ISO 8601's signed extended-year representation as well.
        format!("{year:+05}")
    };
    write!(
        output,
        "-{:02}-{:02}T{:02}:{:02}:{:02}",
        u8::from(timestamp.month()),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
    )
    .expect("writing to a String cannot fail");
    let nanos = timestamp.nanosecond();
    if nanos != 0 {
        if nanos.is_multiple_of(1_000_000) {
            write!(output, ".{:03}", nanos / 1_000_000)
        } else if nanos.is_multiple_of(1_000) {
            write!(output, ".{:06}", nanos / 1_000)
        } else {
            write!(output, ".{nanos:09}")
        }
        .expect("writing to a String cannot fail");
    }
    output.push_str("+00:00");
    output
}

/// ISO 8601 timeout text, without converting or truncating the actual duration.
#[cfg(any(feature = "client", feature = "server"))]
pub(crate) struct IsoDuration(pub(crate) std::time::Duration);

#[cfg(any(feature = "client", feature = "server"))]
impl std::fmt::Display for IsoDuration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_zero() {
            return formatter.write_str("P0D");
        }
        write!(formatter, "PT{}", self.0.as_secs())?;
        let mut nanos = self.0.subsec_nanos();
        if nanos != 0 {
            let mut digits = 9;
            while nanos.is_multiple_of(10) {
                nanos /= 10;
                digits -= 1;
            }
            write!(formatter, ".{nanos:0digits$}")?;
        }
        formatter.write_str("S")
    }
}

#[cfg(feature = "request-state")]
pub(crate) fn unix_milliseconds(timestamp: OffsetDateTime) -> i64 {
    // Euclidean division retains floor rounding for instants before the epoch.
    // The full `time` date range fits in an i64 number of milliseconds.
    i64::try_from(timestamp.unix_timestamp_nanos().div_euclid(1_000_000))
        .expect("a representable timestamp fits in Unix milliseconds")
}

#[cfg(test)]
mod tests {
    use time::format_description::well_known::Rfc3339;

    use super::*;

    #[test]
    fn timestamp_keeps_utc_offset_and_previous_fractional_precision() {
        for (nanos, expected) in [
            (0, "1970-01-01T00:00:00+00:00"),
            (100_000_000, "1970-01-01T00:00:00.100+00:00"),
            (100_000, "1970-01-01T00:00:00.000100+00:00"),
            (100, "1970-01-01T00:00:00.000000100+00:00"),
            (123_456_789, "1970-01-01T00:00:00.123456789+00:00"),
        ] {
            let timestamp = OffsetDateTime::UNIX_EPOCH
                .replace_nanosecond(nanos)
                .unwrap();
            let encoded = rfc3339_utc(timestamp);
            assert_eq!(encoded, expected);
            assert_eq!(
                OffsetDateTime::parse(&encoded, &Rfc3339).unwrap(),
                timestamp
            );
        }
    }

    #[test]
    fn timestamp_normalizes_offsets_and_preserves_pre_epoch_instants() {
        let timestamp =
            OffsetDateTime::parse("1969-12-31T18:59:59.999999999-05:00", &Rfc3339).unwrap();
        assert_eq!(
            rfc3339_utc(timestamp),
            "1969-12-31T23:59:59.999999999+00:00"
        );
    }

    #[test]
    fn extended_years_keep_the_iso8601_sign() {
        let timestamp = time::Date::from_calendar_date(10_000, time::Month::January, 1)
            .unwrap()
            .midnight()
            .assume_utc();
        assert_eq!(rfc3339_utc(timestamp), "+10000-01-01T00:00:00+00:00");
    }

    #[cfg(any(feature = "client", feature = "server"))]
    #[test]
    fn timeout_duration_keeps_iso8601_precision() {
        use std::time::Duration;

        for (timeout, expected) in [
            (Duration::ZERO, "P0D"),
            (Duration::from_secs(42), "PT42S"),
            (Duration::from_millis(42), "PT0.042S"),
            (Duration::from_micros(42), "PT0.000042S"),
            (Duration::from_nanos(42), "PT0.000000042S"),
        ] {
            assert_eq!(IsoDuration(timeout).to_string(), expected);
        }
        // Large standard-library durations must not silently turn into zero.
        assert_eq!(
            IsoDuration(Duration::MAX).to_string(),
            "PT18446744073709551615.999999999S",
        );
    }

    #[cfg(feature = "request-state")]
    #[test]
    fn millisecond_clock_keeps_expiry_rounding_on_both_sides_of_epoch() {
        for (nanos, milliseconds) in [
            (-1_000_001, -2),
            (-1_000_000, -1),
            (-1, -1),
            (0, 0),
            (999_999, 0),
            (1_000_000, 1),
        ] {
            assert_eq!(
                unix_milliseconds(OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap()),
                milliseconds,
            );
        }
    }
}
