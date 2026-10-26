//! RNF-14: montar e desmontar superthreads nunca perde threads nem vínculos (teste de propriedades).

use novo_graph::{Graph, is_superthread};
use proptest::prelude::*;
use std::collections::{BTreeSet, HashMap};

const THREADS: usize = 8;

#[derive(Clone, Debug)]
enum Action {
    Relate(usize, usize),
    RelateProject(usize, usize),
    Disassemble(usize),
    Delete(usize),
    Undo,
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        4 => (0..THREADS + 4, 0..THREADS + 4).prop_map(|(a, b)| Action::Relate(a, b)),
        2 => (0..THREADS + 4, 0..2usize).prop_map(|(a, p)| Action::RelateProject(a, p)),
        3 => (0..THREADS + 4).prop_map(Action::Disassemble),
        1 => (0..THREADS).prop_map(Action::Delete),
        1 => Just(Action::Undo),
    ]
}

/// Uma peça pelo índice: as primeiras são threads, as demais superthreads que existem no momento.
fn piece(g: &Graph, i: usize) -> Option<String> {
    if i < THREADS {
        return Some(format!("t-{i}"));
    }
    g.superthreads().into_iter().nth(i - THREADS)
}

/// Projetos que uma thread alcança: os dela e os dos seus ancestrais.
fn reach(g: &Graph, t: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut cur = Some(t.to_string());
    while let Some(c) = cur {
        out.extend(g.relations(&c).into_iter().map(|(_, to)| to));
        cur = g.parent(&c);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn composition_never_loses_links(actions in prop::collection::vec(action(), 1..25)) {
        let mut g = Graph::default();
        let mut n = 0;
        let mut deleted: Vec<(String, Vec<novo_graph::EdgeEvent>)> = vec![];
        let mut gone: BTreeSet<String> = BTreeSet::new();
        for a in actions {
            let before: HashMap<String, BTreeSet<String>> = (0..THREADS).map(|i| format!("t-{i}")).map(|t| (t.clone(), reach(&g, &t))).collect();
            let mut moved: BTreeSet<String> = BTreeSet::new();
            match a {
                Action::Relate(x, y) => {
                    let (Some(x), Some(y)) = (piece(&g, x), piece(&g, y)) else { continue };
                    if gone.contains(&x) || gone.contains(&y) { continue }
                    if let Ok(ch) = g.relate(&x, &y, || { n += 1; format!("st-{n:04}") }) {
                        for e in &ch.events { g.apply(e); }
                    }
                }
                Action::RelateProject(x, p) => {
                    let Some(x) = piece(&g, x) else { continue };
                    if gone.contains(&x) { continue }
                    if let Ok(ch) = g.relate(&x, &format!("p-{p}"), || unreachable!()) {
                        for e in &ch.events { g.apply(e); }
                    }
                }
                Action::Disassemble(x) => {
                    let Some(x) = piece(&g, x) else { continue };
                    if let Ok(ch) = g.disassemble(&x) {
                        moved.insert(x.clone());
                        if is_superthread(&x) { moved.extend(g.leaf_threads(&x)); }
                        for e in &ch.events { g.apply(e); }
                    }
                }
                Action::Delete(i) => {
                    let t = format!("t-{i}");
                    if gone.contains(&t) { continue }
                    let ch = g.detach_node(&t);
                    for e in &ch.events { g.apply(e); }
                    gone.insert(t.clone());
                    moved.insert(t.clone());
                    deleted.push((t, ch.events));
                }
                Action::Undo => {
                    if let Some((t, events)) = deleted.pop() {
                        // só desfaz se nada mudou desde a exclusão (como o app faz: desfazer imediato)
                        let mut sim = g.clone();
                        for e in events.iter().rev() { sim.apply(&e.inverse()); }
                        if sim.check().is_ok() { g = sim; gone.remove(&t); }
                    }
                }
            }
            prop_assert!(g.check().is_ok(), "invariante quebrado: {:?}", g.check());
            for i in 0..THREADS {
                let t = format!("t-{i}");
                if moved.contains(&t) || gone.contains(&t) { continue }
                let after = reach(&g, &t);
                prop_assert!(before[&t].is_subset(&after), "{t} perdeu vínculos: {:?} -> {:?}", before[&t], after);
            }
        }
    }
}

#[test]
fn ten_thousand_random_operations_keep_invariants() {
    // RNF-14: 10 mil operações aleatórias determinísticas
    let mut g = Graph::default();
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    let mut rnd = |m: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as usize
    };
    let mut n = 0;
    for _ in 0..10_000 {
        match rnd(3) {
            0 => {
                let (a, b) = (piece(&g, rnd(THREADS + 4)), piece(&g, rnd(THREADS + 4)));
                if let (Some(a), Some(b)) = (a, b) {
                    if let Ok(ch) = g.relate(&a, &b, || { n += 1; format!("st-{n:05}") }) {
                        for e in &ch.events { g.apply(e); }
                    }
                }
            }
            1 => {
                if let Some(a) = piece(&g, rnd(THREADS + 4)) {
                    if let Ok(ch) = g.disassemble(&a) {
                        for e in &ch.events { g.apply(e); }
                    }
                }
            }
            _ => {
                if let Some(a) = piece(&g, rnd(THREADS + 4)) {
                    if let Ok(ch) = g.relate(&a, &format!("p-{}", rnd(3)), || unreachable!()) {
                        for e in &ch.events { g.apply(e); }
                    }
                }
            }
        }
        g.check().unwrap();
    }
    // nenhuma thread some: todas continuam soltas ou dentro de exatamente uma árvore
    for i in 0..THREADS {
        let t = format!("t-{i}");
        let top = g.top(&t);
        assert!(top == t || g.leaf_threads(&top).contains(&t));
    }
}
