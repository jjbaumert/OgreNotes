use chrono::{DateTime, Utc};

/// Return the current time as microseconds since Unix epoch.
pub fn now_usec() -> i64 {
    Utc::now().timestamp_micros()
}

/// Convert microseconds since epoch to a `DateTime<Utc>`.
pub fn usec_to_datetime(usec: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_micros(usec)
}

/// Convert a `DateTime<Utc>` to microseconds since epoch.
pub fn datetime_to_usec(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_micros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_microsecond_precision() {
        let usec = now_usec();
        // Should be a reasonable timestamp (after 2020, in microseconds)
        assert!(usec > 1_577_836_800_000_000); // 2020-01-01 in usec
    }

    #[test]
    fn timestamp_roundtrip() {
        let usec = now_usec();
        let dt = usec_to_datetime(usec).expect("valid timestamp");
        let back = datetime_to_usec(dt);
        assert_eq!(usec, back);
    }

    #[test]
    fn timestamp_ordering() {
        let a = now_usec();
        // Spin briefly to ensure time advances
        std::hint::spin_loop();
        let b = now_usec();
        assert!(b >= a);
    }
}
