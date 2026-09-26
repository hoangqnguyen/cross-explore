//! Timestamp conversions. Zip stores zone-less MS-DOS times; we treat them as
//! UTC both when writing and when reading (and also write the "UT" extra
//! field, which carries a real UTC time and wins when present).

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn secs_to_ms(s: i64) -> i64 {
    s.saturating_mul(1000)
}

pub(crate) fn system_to_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

pub(crate) fn now_ms() -> i64 {
    system_to_ms(SystemTime::now())
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub(crate) fn dos_to_ms(t: zip::DateTime) -> i64 {
    let days = days_from_civil(t.year() as i64, t.month() as u32, t.day() as u32);
    let secs = days * 86400 + t.hour() as i64 * 3600 + t.minute() as i64 * 60 + t.second() as i64;
    secs_to_ms(secs)
}

/// The DOS time for `ms`, clamped to the representable 1980–2107 range.
pub(crate) fn ms_to_dos(ms: i64) -> zip::DateTime {
    let secs = ms.div_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86400));
    let sod = secs.rem_euclid(86400);
    let (h, mi, s) = ((sod / 3600) as u8, ((sod % 3600) / 60) as u8, (sod % 60) as u8);
    if y < 1980 {
        return zip::DateTime::default();
    }
    let y = y.min(2107) as u16;
    zip::DateTime::from_date_and_time(y, mo as u8, d as u8, h, mi, s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_round_trip() {
        let ms = 1_700_000_000_000; // 2023-11-14T22:13:20Z
        let dos = ms_to_dos(ms);
        assert_eq!((dos.year(), dos.month(), dos.day(), dos.hour(), dos.minute()), (2023, 11, 14, 22, 13));
        assert_eq!(dos_to_ms(dos), ms);
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
    }
}
