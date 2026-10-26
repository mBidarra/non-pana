//! Adição rápida em linguagem natural (RF-TSK-05): "revisar landing amanhã 9h p1 #marca @rapido".
//! `#nome` = projeto (se existir um com esse nome) ou etiqueta; `@nome` = etiqueta; `p1`..`p4` = prioridade.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use novo_core::text::slug;

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub due: Option<String>,
    pub priority: Option<u8>,
    pub labels: Vec<String>,
    pub project: Option<String>,
}

fn weekday(w: &str) -> Option<Weekday> {
    Some(match w {
        "segunda" | "seg" => Weekday::Mon,
        "terça" | "terca" | "ter" => Weekday::Tue,
        "quarta" | "qua" => Weekday::Wed,
        "quinta" | "qui" => Weekday::Thu,
        "sexta" | "sex" => Weekday::Fri,
        "sábado" | "sabado" | "sab" => Weekday::Sat,
        "domingo" | "dom" => Weekday::Sun,
        _ => return None,
    })
}

fn time_of(w: &str) -> Option<(u32, u32)> {
    let w = w.trim_end_matches("min");
    if let Some((h, m)) = w.split_once('h') {
        let h: u32 = h.parse().ok()?;
        let m: u32 = if m.is_empty() { 0 } else { m.parse().ok()? };
        return (h < 24 && m < 60).then_some((h, m));
    }
    if let Some((h, m)) = w.split_once(':') {
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        return (h < 24 && m < 60).then_some((h, m));
    }
    None
}

/// `projects`: (id, nome) dos projetos existentes.
pub fn parse(text: &str, projects: &[(String, String)], today: NaiveDate) -> Parsed {
    let mut p = Parsed::default();
    let mut date: Option<NaiveDate> = None;
    let mut hm: Option<(u32, u32)> = None;
    let mut words = vec![];
    for raw in text.split_whitespace() {
        let w = raw.to_lowercase();
        if let Some(rest) = w.strip_prefix('#') {
            let s = slug(rest, 60);
            if let Some((id, _)) = projects.iter().find(|(id, name)| slug(name, 60) == s || id == rest) {
                p.project = Some(id.clone());
            } else if !s.is_empty() {
                p.labels.push(s);
            }
            continue;
        }
        if let Some(rest) = w.strip_prefix('@') {
            if !rest.is_empty() {
                p.labels.push(slug(rest, 60));
            }
            continue;
        }
        if matches!(w.as_str(), "p1" | "p2" | "p3" | "p4") {
            p.priority = w[1..].parse().ok();
            continue;
        }
        match w.as_str() {
            "hoje" => {
                date = Some(today);
                continue;
            }
            "amanhã" | "amanha" => {
                date = Some(today + Duration::days(1));
                continue;
            }
            "depois-de-amanhã" | "depois-de-amanha" => {
                date = Some(today + Duration::days(2));
                continue;
            }
            _ => {}
        }
        if let Some(wd) = weekday(&w) {
            let mut d = today + Duration::days(1);
            while d.weekday() != wd {
                d += Duration::days(1);
            }
            date = Some(d);
            continue;
        }
        if let Some(t) = time_of(&w) {
            hm = Some(t);
            continue;
        }
        if let Ok(d) = NaiveDate::parse_from_str(&w, "%Y-%m-%d") {
            date = Some(d);
            continue;
        }
        if let Some((dd, mm)) = w.split_once('/') {
            if let (Ok(dd), Ok(mm)) = (dd.parse::<u32>(), mm.parse::<u32>()) {
                if let Some(mut d) = NaiveDate::from_ymd_opt(today.year(), mm, dd) {
                    if d < today {
                        d = NaiveDate::from_ymd_opt(today.year() + 1, mm, dd).unwrap_or(d);
                    }
                    date = Some(d);
                    continue;
                }
            }
        }
        words.push(raw.to_string());
    }
    if hm.is_some() && date.is_none() {
        date = Some(today);
    }
    p.due = date.map(|d| match hm {
        Some((h, m)) => format!("{}T{:02}:{:02}:00-03:00", d.format("%Y-%m-%d"), h, m),
        None => d.format("%Y-%m-%d").to_string(),
    });
    p.title = words.join(" ");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(); // segunda
        let projects = vec![("p-1".to_string(), "Marca".to_string())];
        let r = parse("revisar landing amanhã 9h p1 #marca", &projects, today);
        assert_eq!(r.title, "revisar landing");
        assert_eq!(r.due.as_deref(), Some("2026-10-27T09:00:00-03:00"));
        assert_eq!(r.priority, Some(1));
        assert_eq!(r.project.as_deref(), Some("p-1"));
        let r = parse("ligar sexta @rapido #sem-projeto", &projects, today);
        assert_eq!(r.due.as_deref(), Some("2026-10-30"));
        assert_eq!(r.labels, vec!["rapido", "sem-projeto"]);
    }
}
