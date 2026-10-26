//! Critério de pronto (spec 10.1): 14 relatórios do Foco seguidos, incluindo noites com o Mac desligado.
//! Binário de teste próprio porque muda o relógio do processo (`NOVO_FAKE_NOW`) e o fuso (`TZ`).

use chrono::{Duration, NaiveDate};
use novo_core::Vault;
use novo_service::{Service, focus_ops, rpc};
use serde_json::json;
use std::sync::Arc;

fn set_now(day: NaiveDate, hhmm: &str) {
    unsafe { std::env::set_var("NOVO_FAKE_NOW", format!("{day}T{hhmm}:00-03:00")) };
}

#[tokio::test]
async fn fourteen_nights_in_a_row_with_the_mac_off_on_some() {
    unsafe {
        std::env::set_var("NOVO_CLAUDE_BIN", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_claude.sh"));
        std::env::set_var("NOVO_NO_NOTIFY", "1");
        // o Mac em outro fuso não muda o dia das métricas (RNF-08)
        std::env::set_var("TZ", "Asia/Tokyo");
    }
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let d0 = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    std::fs::write(vault.runtime_dir().join("created"), d0.to_string()).unwrap();
    let svc = Arc::new(Service::open(vault).unwrap());

    // noites com o Mac desligado: o daemon não roda à meia-noite
    let off = [4, 5, 6, 11];
    let mut recovered = vec![];
    for night in 1..=14i64 {
        let day = d0 + Duration::days(night - 1);
        set_now(day, "12:00");
        rpc::call(&svc, "thread.create", json!({"text": format!("pensamento do dia {night}")})).await.unwrap();
        if off.contains(&night) {
            continue;
        }
        set_now(day + Duration::days(1), "00:05");
        let days = focus_ops::catch_up(&svc).await.unwrap();
        assert_eq!(days.last(), Some(&day), "a noite {night} gera o relatório do dia que terminou");
        if days.len() > 1 {
            recovered.push((night, days.len()));
        }
    }
    assert_eq!(recovered, vec![(7, 4), (12, 2)], "ao ligar, os dias perdidos saem junto com o do dia");

    let list = rpc::call(&svc, "focus.list", json!({})).await.unwrap();
    let days: Vec<String> = list.as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect();
    let expected: Vec<String> = (0..14).map(|i| (d0 + Duration::days(i)).to_string()).collect();
    assert_eq!(days, expected, "14 relatórios seguidos, sem buraco");
    for day in &expected {
        let r = rpc::call(&svc, "focus.get", json!({"day": day})).await.unwrap();
        assert_eq!(r["counts"]["threads_created"], 1, "cada relatório conta só o próprio dia ({day})");
        assert_eq!(r["window_start"].as_str().unwrap(), format!("{day}T00:00:00-03:00"), "janela 00:00 GMT-3");
    }
}
