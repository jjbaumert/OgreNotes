// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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

    #[test]
    fn epoch_maps_to_unix_zero() {
        // Zero microseconds is exactly the Unix epoch — pins the unit
        // (microseconds, not millis or nanos) and the epoch base.
        let dt = usec_to_datetime(0).expect("epoch is valid");
        assert_eq!(dt.to_rfc3339(), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn known_timestamp_maps_to_known_instant() {
        // 1_700_000_000 seconds = 2023-11-14T22:13:20Z. If the unit were
        // milliseconds or nanoseconds this would land centuries away, so
        // this test catches any unit confusion in the helpers.
        let dt = usec_to_datetime(1_700_000_000_000_000).expect("valid timestamp");
        assert_eq!(dt.to_rfc3339(), "2023-11-14T22:13:20+00:00");
    }

    #[test]
    fn subsecond_microseconds_are_preserved() {
        // created_at ordering in DynamoDB depends on full microsecond
        // precision surviving the conversion.
        let usec = 1_700_000_000_123_456;
        let dt = usec_to_datetime(usec).expect("valid timestamp");
        assert_eq!(dt.timestamp_subsec_micros(), 123_456);
        assert_eq!(datetime_to_usec(dt), usec);
    }

    #[test]
    fn pre_epoch_timestamp_roundtrips() {
        // Negative values are valid (pre-1970) and must round-trip, not
        // saturate or wrap.
        let dt = usec_to_datetime(-1_000_000).expect("pre-epoch is valid");
        assert_eq!(dt.to_rfc3339(), "1969-12-31T23:59:59+00:00");
        assert_eq!(datetime_to_usec(dt), -1_000_000);
    }

    #[test]
    fn out_of_range_returns_none_not_panic() {
        // chrono's representable range is far narrower than i64 micros;
        // a corrupt or attacker-supplied timestamp must yield None, never
        // a panic in this L1 helper.
        assert!(usec_to_datetime(i64::MAX).is_none());
        assert!(usec_to_datetime(i64::MIN).is_none());
    }

    proptest::proptest! {
        #[test]
        fn roundtrip_holds_across_representable_range(
            // Years ~1000..~3000 in microseconds — comfortably inside
            // chrono's representable range, covering all realistic data.
            usec in -30_610_224_000_000_000i64..=32_503_680_000_000_000i64
        ) {
            let dt = usec_to_datetime(usec).expect("in-range timestamp");
            proptest::prop_assert_eq!(datetime_to_usec(dt), usec);
        }
    }
}
