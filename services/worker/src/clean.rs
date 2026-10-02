//! Data cleaning, rules version 1 (spec `market/public-worker` "数据清洗单元的执行"; design D10).
//!
//! The result depends only on the input bytes, so honest workers on any machine produce the same
//! bytes and the same summary:
//!
//! 1. Unicode NFC;
//! 2. control characters removed, except line feed and tab;
//! 3. in every line, runs of whitespace become one space and the line is trimmed; leading and
//!    trailing empty lines are dropped;
//! 4. documents shorter than [`MIN_CHARS`] characters after this are dropped;
//! 5. exact duplicates (equal normalized text) are dropped, the first kept;
//! 6. near duplicates are dropped, the first kept: MinHash over lower-cased character 5-grams
//!    ([`PERMUTATIONS`] permutations from a fixed seed), banded for candidates ([`BANDS`] bands of
//!    [`ROWS`] rows) and confirmed when at least [`SAME_MINIMA`] minima are equal (an estimated
//!    Jaccard similarity of at least 0.8).
//!
//! The full result is the kept documents as JSON Lines `{"index":i,"text":t}` in input order;
//! the summary is its BLAKE3 hash.

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;

/// Shortest kept document, in characters.
pub const MIN_CHARS: usize = 32;
/// Characters per shingle.
pub const SHINGLE: usize = 5;
/// MinHash permutations.
pub const PERMUTATIONS: usize = 128;
/// LSH bands.
pub const BANDS: usize = 16;
/// Rows per band.
pub const ROWS: usize = 8;
/// Equal minima that confirm a near duplicate: ⌈0.8 × 128⌉.
pub const SAME_MINIMA: usize = 103;

/// The Mersenne prime 2^61 − 1 the permutations work modulo.
const P: u64 = (1 << 61) - 1;

/// Normalizes a document's text (steps 1–3).
#[must_use]
pub fn normalize(text: &str) -> String {
    let nfc: String = text
        .nfc()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let lines: Vec<String> = nfc
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let first = lines.iter().position(|l| !l.is_empty());
    let last = lines.iter().rposition(|l| !l.is_empty());
    match (first, last) {
        (Some(f), Some(l)) => lines.get(f..=l).unwrap_or_default().join("\n"),
        _ => String::new(),
    }
}

/// SplitMix64: the fixed source of the permutation coefficients (not a security primitive; it
/// only has to be the same everywhere).
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The `(a, b)` coefficients of the permutations `x ↦ a·x + b mod P`.
fn coefficients() -> Vec<(u64, u64)> {
    let mut state = 0x6167_656e_7463_6f69; // "agentcoi"
    (0..PERMUTATIONS)
        .map(|_| {
            let a = splitmix(&mut state) % (P - 1) + 1;
            let b = splitmix(&mut state) % P;
            (a, b)
        })
        .collect()
}

fn mod_p(x: u128) -> u64 {
    // P < 2^61, so the remainder fits.
    u64::try_from(x % u128::from(P)).unwrap_or(0)
}

/// The MinHash signature of a normalized text.
#[must_use]
pub fn signature(text: &str, coeffs: &[(u64, u64)]) -> Vec<u64> {
    let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut shingles: BTreeSet<u64> = BTreeSet::new();
    let windows: Vec<&[char]> = if chars.len() < SHINGLE {
        vec![&chars[..]]
    } else {
        chars.windows(SHINGLE).collect()
    };
    for w in windows {
        let s: String = w.iter().collect();
        let h = ac_crypto::hash::blake3_256(s.as_bytes());
        let mut word = [0u8; 8];
        word.copy_from_slice(h.get(..8).unwrap_or(&[0; 8]));
        shingles.insert(u64::from_le_bytes(word) % P);
    }
    coeffs
        .iter()
        .map(|(a, b)| {
            shingles
                .iter()
                .map(|x| mod_p(u128::from(*a) * u128::from(*x) + u128::from(*b)))
                .min()
                .unwrap_or(u64::MAX)
        })
        .collect()
}

fn band_key(sig: &[u64], band: usize) -> (usize, Vec<u64>) {
    let rows = sig
        .get(band * ROWS..(band + 1) * ROWS)
        .unwrap_or_default()
        .to_vec();
    (band, rows)
}

/// A kept document.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    /// Position in the unit's input.
    pub index: usize,
    /// Normalized text.
    pub text: String,
}

/// Cleans a unit's documents (rules v1) and returns the kept ones in input order.
#[must_use]
pub fn clean(docs: &[String]) -> Vec<Kept> {
    let coeffs = coefficients();
    let mut seen: BTreeSet<[u8; 32]> = BTreeSet::new();
    let mut bands: BTreeMap<(usize, Vec<u64>), Vec<usize>> = BTreeMap::new();
    let mut signatures: Vec<Vec<u64>> = Vec::new();
    let mut kept = Vec::new();
    for (index, doc) in docs.iter().enumerate() {
        let text = normalize(doc);
        if text.chars().count() < MIN_CHARS {
            continue;
        }
        if !seen.insert(ac_crypto::hash::blake3_256(text.as_bytes())) {
            continue;
        }
        let sig = signature(&text, &coeffs);
        let mut candidates: BTreeSet<usize> = BTreeSet::new();
        for band in 0..BANDS {
            if let Some(list) = bands.get(&band_key(&sig, band)) {
                candidates.extend(list.iter().copied());
            }
        }
        let near = candidates.iter().any(|k| {
            signatures.get(*k).is_some_and(|other| {
                other.iter().zip(&sig).filter(|(a, b)| a == b).count() >= SAME_MINIMA
            })
        });
        if near {
            continue;
        }
        let slot = signatures.len();
        for band in 0..BANDS {
            bands.entry(band_key(&sig, band)).or_default().push(slot);
        }
        signatures.push(sig);
        kept.push(Kept { index, text });
    }
    kept
}

/// The full result: kept documents as JSON Lines.
///
/// # Errors
///
/// Serialization failure (none in practice).
pub fn result_bytes(kept: &[Kept]) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::new();
    for k in kept {
        serde_json::to_writer(&mut out, k)?;
        out.push(b'\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // Test code.

    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("  a \t  b  \n\n c\u{7}d  \n\n"), "a b\n\ncd");
        // NFC: e + combining acute becomes é.
        assert_eq!(normalize("e\u{301}"), "\u{e9}");
        assert_eq!(normalize(" \n \n"), "");
    }

    #[test]
    fn short_and_exact_duplicate_documents_are_dropped() {
        let long = "The quick brown fox jumps over the lazy dog near the river bank.";
        let kept = clean(&s(&["too short", long, &format!("  {long}  "), long]));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].index, 1);
    }

    // Spec "近似重复被删除": two documents that differ by one punctuation mark.
    #[test]
    fn near_duplicates_are_dropped() {
        let a = "Hash functions map data of any size to values of a fixed size, quickly and repeatably.";
        let b = "Hash functions map data of any size to values of a fixed size; quickly and repeatably.";
        let c =
            "Pancakes need flour, milk and eggs, a hot pan, and a little patience in the morning.";
        let kept = clean(&s(&[a, b, c]));
        assert_eq!(kept.iter().map(|k| k.index).collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn signatures_are_deterministic_and_similar_texts_share_minima() {
        let coeffs = coefficients();
        let x = signature("abcdefghijklmnopqrstuvwxyz", &coeffs);
        assert_eq!(x, signature("abcdefghijklmnopqrstuvwxyz", &coeffs));
        assert_eq!(x.len(), PERMUTATIONS);
        let y = signature("zyxwvutsrqponmlkjihgfedcba", &coeffs);
        assert!(x.iter().zip(&y).filter(|(a, b)| a == b).count() < SAME_MINIMA);
    }
}
