//! IDs ordenáveis: prefixo + 10 caracteres base32 (42 bits de tempo em ms + 8 bits de desempate).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
static COUNTER: AtomicU64 = AtomicU64::new(0);
static LAST: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn random_u64() -> u64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.finish()
}

/// Novo id com o prefixo dado (`t`, `st`, `k`, `p`, `n`, `c`).
/// Monotônico dentro do processo: dois ids no mesmo ms nunca colidem nem invertem a ordem.
pub fn new_id(prefix: &str) -> String {
    let mut value = (now_ms() << 8) | (random_u64() & 0xff);
    loop {
        let last = LAST.load(Ordering::SeqCst);
        if value <= last {
            value = last + 1;
        }
        if LAST.compare_exchange(last, value, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            break;
        }
    }
    format!("{prefix}-{}", encode(value))
}

pub fn encode(mut v: u64) -> String {
    let mut out = [b'0'; 10];
    for i in (0..10).rev() {
        out[i] = ALPHABET[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

/// Um id de item do cofre (`t-…`, `st-…`, `k-…`, `p-…`, `n-…`, `c-…`): só minúsculas, dígitos e hífens.
/// Barra, ponto e `..` nunca passam, então um id nunca vira caminho fora da pasta dele.
pub fn is_valid(id: &str) -> bool {
    let Some((prefix, rest)) = id.split_once('-') else { return false };
    matches!(prefix, "t" | "st" | "k" | "p" | "n" | "c")
        && !rest.is_empty()
        && id.len() <= 160
        && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Milissegundos codificados num id, quando ele segue o formato.
pub fn millis_of(id: &str) -> Option<u64> {
    let tail = id.rsplit('-').next()?;
    if tail.len() != 10 {
        return None;
    }
    let mut v: u64 = 0;
    for c in tail.bytes() {
        let d = ALPHABET.iter().position(|&a| a == c)? as u64;
        v = (v << 5) | d;
    }
    Some(v >> 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sort_by_creation_and_decode_time() {
        let a = new_id("t");
        let b = new_id("t");
        assert!(a < b, "{a} < {b}");
        let ms = millis_of(&a).unwrap();
        assert!(ms.abs_diff(now_ms()) < 5_000);
        assert_eq!(a.len(), 12);
    }
}
