//! UTC timestamps for backups, without a date crate: seconds since the epoch
//! to a civil date (Howard Hinnant's `civil_from_days`).

use std::time::{SystemTime, UNIX_EPOCH};

/// A UTC time to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Utc {
    /// Times before 1970 clamp to the epoch.
    pub fn from_system(t: SystemTime) -> Self {
        let secs = t
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self::from_unix(secs)
    }

    pub fn from_unix(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        Self {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    /// RFC 3339: `2026-10-17T16:57:03Z`.
    pub fn rfc3339(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// For file names, sortable and colon-free: `20261017-165703`.
    pub fn compact(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(Utc::from_unix(0).rfc3339(), "1970-01-01T00:00:00Z");
        assert_eq!(
            Utc::from_unix(951_782_400).rfc3339(),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            Utc::from_unix(1_792_256_223).rfc3339(),
            "2026-10-17T16:57:03Z"
        );
        assert_eq!(Utc::from_unix(1_792_256_223).compact(), "20261017-165703");
        assert_eq!(
            Utc::from_unix(4_102_444_799).rfc3339(),
            "2099-12-31T23:59:59Z"
        );
    }
}
