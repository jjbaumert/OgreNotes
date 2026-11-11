// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Target-specific primitives. Browser evaluation deliberately keeps using the
//! browser's Date parser, timezone and random generator. Native exports use UTC
//! and a documented ISO-only parser instead of depending on the server timezone.

#[cfg(target_arch = "wasm32")]
pub(crate) use js_sys::Date;

#[cfg(target_arch = "wasm32")]
pub(crate) fn parse_date(value: &str) -> Date {
    Date::new(&value.into())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn random() -> f64 {
    js_sys::Math::random()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{Date, parse_date};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn random() -> f64 {
    rand::random()
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::Cell;

    use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};

    /// UTC counterpart of the small JavaScript Date surface used by formulas.
    /// Invalid inputs retain an invalid timestamp and zero-valued integer
    /// getters, matching the existing js-sys integer getter conversions.
    pub(crate) struct Date(Cell<Option<NaiveDateTime>>);

    pub(crate) fn parse_date(value: &str) -> Date {
        let value = value.trim();
        let parsed = DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|date| date.naive_utc())
            .or_else(|| {
                [
                    "%Y-%m-%dT%H:%M:%S%.f",
                    "%Y-%m-%d %H:%M:%S%.f",
                    "%Y-%m-%dT%H:%M",
                    "%Y-%m-%d %H:%M",
                ]
                .iter()
                .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
            })
            .or_else(|| {
                NaiveDate::parse_from_str(value, "%Y-%m-%d")
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
            });
        Date(Cell::new(parsed))
    }

    /// Construct the first of the requested month, then add the day offset.
    /// This preserves JavaScript's overflow semantics for e.g. month 12 or day
    /// zero, including the sequential year/month setters used by EDATE.
    fn normalized_date(year: i32, month: i32, day: i64) -> Option<NaiveDate> {
        let year = year.checked_add(month.div_euclid(12))?;
        let month = month.rem_euclid(12) as u32 + 1;
        NaiveDate::from_ymd_opt(year, month, 1)?
            .checked_add_signed(Duration::try_days(day.checked_sub(1)?)?)
    }

    impl Date {
        pub(crate) fn new_0() -> Self {
            Self(Cell::new(Some(Utc::now().naive_utc())))
        }

        pub(crate) fn new_with_year_month_day(year: u32, month: i32, day: i32) -> Self {
            let year = i32::try_from(year).ok().and_then(|year| {
                if (0..=99).contains(&year) {
                    year.checked_add(1900)
                } else {
                    Some(year)
                }
            });
            Self(Cell::new(
                year.and_then(|year| normalized_date(year, month, i64::from(day)))
                    .and_then(|date| date.and_hms_opt(0, 0, 0)),
            ))
        }

        pub(crate) fn get_full_year(&self) -> u32 {
            self.0.get().map_or(0, |date| date.year() as u32)
        }

        pub(crate) fn get_month(&self) -> u32 {
            self.0.get().map_or(0, |date| date.month0())
        }

        pub(crate) fn get_date(&self) -> u32 {
            self.0.get().map_or(0, |date| date.day())
        }

        pub(crate) fn get_day(&self) -> u32 {
            self.0
                .get()
                .map_or(0, |date| date.weekday().num_days_from_sunday())
        }

        pub(crate) fn get_time(&self) -> f64 {
            self.0
                .get()
                .map_or(f64::NAN, |date| date.and_utc().timestamp_millis() as f64)
        }

        fn replace_date(&self, date: Option<NaiveDate>, time: NaiveTime) -> f64 {
            self.0.set(date.map(|date| date.and_time(time)));
            self.get_time()
        }

        pub(crate) fn set_full_year(&self, year: u32) -> f64 {
            let Some(current) = self.0.get() else {
                return f64::NAN;
            };
            let next = i32::try_from(year).ok().and_then(|year| {
                normalized_date(year, current.month0() as i32, i64::from(current.day()))
            });
            self.replace_date(next, current.time())
        }

        pub(crate) fn set_month(&self, month: u32) -> f64 {
            let Some(current) = self.0.get() else {
                return f64::NAN;
            };
            let next = i32::try_from(month)
                .ok()
                .and_then(|month| normalized_date(current.year(), month, i64::from(current.day())));
            self.replace_date(next, current.time())
        }

        pub(crate) fn set_date(&self, day: u32) -> f64 {
            let Some(current) = self.0.get() else {
                return f64::NAN;
            };
            let next = normalized_date(current.year(), current.month0() as i32, i64::from(day));
            self.replace_date(next, current.time())
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{Date, parse_date, random};
    use crate::eval::{CellValue, SpreadsheetEngine};

    #[test]
    fn native_dates_use_utc_for_explicit_offsets() {
        let date = parse_date("2026-10-28T23:30:00-04:00");
        assert_eq!(
            (date.get_full_year(), date.get_month(), date.get_date()),
            (2026, 9, 29)
        );
        assert_eq!(date.get_day(), 4);
    }

    #[test]
    fn native_date_setters_preserve_overflow_and_day_zero() {
        let date = parse_date("2026-01-31");
        date.set_month(1);
        assert_eq!((date.get_month(), date.get_date()), (2, 3));
        date.set_date(0);
        assert_eq!((date.get_month(), date.get_date()), (1, 28));
        let year = Date::new_with_year_month_day(26, 0, 1);
        assert_eq!(year.get_full_year(), 1926);
        let leap = parse_date("2024-02-29");
        leap.set_full_year(2025);
        assert_eq!((leap.get_month(), leap.get_date()), (2, 1));
    }

    #[test]
    fn native_invalid_dates_do_not_panic() {
        assert!(parse_date("not a date").get_time().is_nan());
        assert!(parse_date("2026-02-30").get_time().is_nan());
    }

    #[test]
    fn native_volatile_and_date_formulas_evaluate_without_browser_calls() {
        let mut engine = SpreadsheetEngine::new();
        for (row, formula, expected) in [
            (0, "=WEEKDAY(\"2026-10-28\")", CellValue::Number(4.0)),
            (
                1,
                "=DAYS(\"2026-10-28\",\"2026-10-01\")",
                CellValue::Number(27.0),
            ),
            (
                2,
                "=EDATE(\"2026-01-31\",1)",
                CellValue::Text("2026-03-03".into()),
            ),
            (
                3,
                "=EOMONTH(\"2026-01-15\",1)",
                CellValue::Text("2026-02-28".into()),
            ),
        ] {
            engine.set_cell((0, row), formula);
            assert_eq!(engine.get_value((0, row)), &expected);
        }
        engine.set_cell((1, 0), "=TODAY()");
        assert!(matches!(engine.get_value((1, 0)), CellValue::Text(value) if value.len() == 10));
        engine.set_cell((1, 1), "=RAND()");
        assert!(
            matches!(engine.get_value((1, 1)), CellValue::Number(value) if (0.0..1.0).contains(value))
        );
        assert!((0.0..1.0).contains(&random()));
    }
}
