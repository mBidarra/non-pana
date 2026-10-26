//! Tempo de referência: GMT-3 fixo (America/Sao_Paulo, sem horário de verão).

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone, Utc};

pub type Ts = DateTime<FixedOffset>;

pub fn tz() -> FixedOffset {
    FixedOffset::west_opt(3 * 3600).expect("offset")
}

pub fn now() -> Ts {
    if let Ok(fake) = std::env::var("NOVO_FAKE_NOW") {
        if let Ok(t) = DateTime::parse_from_rfc3339(&fake) {
            return t.with_timezone(&tz());
        }
    }
    Utc::now().with_timezone(&tz())
}

/// O dia (GMT-3) de um instante.
pub fn day_of(ts: &Ts) -> NaiveDate {
    ts.with_timezone(&tz()).date_naive()
}

/// Janela [início, fim) de um dia em GMT-3.
pub fn day_window(day: NaiveDate) -> (Ts, Ts) {
    let start = tz().from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("midnight")).single().expect("fixed offset");
    (start, start + Duration::days(1))
}

pub fn from_millis(ms: u64) -> Ts {
    Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Utc::now).with_timezone(&tz())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_window_is_gmt_minus_3() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let (s, e) = day_window(day);
        assert_eq!(s.to_rfc3339(), "2026-10-26T00:00:00-03:00");
        assert_eq!(e.with_timezone(&Utc).to_rfc3339(), "2026-10-27T03:00:00+00:00");
        let late = DateTime::parse_from_rfc3339("2026-10-27T02:30:00Z").unwrap();
        assert_eq!(day_of(&late), day);
    }
}
