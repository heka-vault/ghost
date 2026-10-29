//! `difflib.SequenceMatcher(None, a, b).ratio()` as CPython computes it: the longest
//! matching blocks found left to right and recursively on either side, summed, times two,
//! over the total length. The matcher's phonetic similarity rests on it, so the numbers
//! have to be the same ones.

use std::collections::HashMap;

/// `SequenceMatcher(None, a, b).ratio()` over code points.
pub fn ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matches: usize = matching_blocks(&a, &b).iter().map(|(_, _, k)| k).sum();
    2.0 * matches as f64 / total as f64
}

fn matching_blocks(a: &[char], b: &[char]) -> Vec<(usize, usize, usize)> {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    // autojunk: in a long b, the elements that make up more than 1 % are left out.
    let n = b.len();
    if n >= 200 {
        let ntest = n / 100 + 1;
        b2j.retain(|_, idxs| idxs.len() <= ntest);
    }
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut matching = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(a, b, &b2j, alo, ahi, blo, bhi);
        if k > 0 {
            matching.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    matching.sort_unstable();
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let (mut i1, mut j1, mut k1) = (0, 0, 0);
    for (i2, j2, k2) in matching {
        if i1 + k1 == i2 && j1 + k1 == j2 {
            k1 += k2;
        } else {
            if k1 > 0 {
                out.push((i1, j1, k1));
            }
            (i1, j1, k1) = (i2, j2, k2);
        }
    }
    if k1 > 0 {
        out.push((i1, j1, k1));
    }
    out
}

fn find_longest_match(
    a: &[char],
    b: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, ai) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut newj2len: HashMap<usize, usize> = HashMap::new();
        if let Some(js) = b2j.get(ai) {
            for &j in js {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = j
                    .checked_sub(1)
                    .and_then(|p| j2len.get(&p))
                    .copied()
                    .unwrap_or(0)
                    + 1;
                newj2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = newj2len;
    }
    // Extend over the elements autojunk left out of b2j (none are junk here).
    while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
        besti -= 1;
        bestj -= 1;
        bestsize += 1;
    }
    while besti + bestsize < ahi
        && bestj + bestsize < bhi
        && a[besti + bestsize] == b[bestj + bestsize]
    {
        bestsize += 1;
    }
    (besti, bestj, bestsize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_match_cpython() {
        // Values from `difflib.SequenceMatcher(None, a, b).ratio()`.
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(ratio("abc", "abc"), 1.0);
        assert_eq!(ratio("abcd", "bcde"), 0.75);
        assert!((ratio("hgst", "hgs") - 6.0 / 7.0).abs() < 1e-12);
        assert!((ratio("gst", "gs") - 0.8).abs() < 1e-12);
        assert!((ratio("kst", "gst") - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(ratio("abc", "xyz"), 0.0);
    }

    #[test]
    fn blocks_are_leftmost_and_merged() {
        let a: Vec<char> = "abxcd".chars().collect();
        let b: Vec<char> = "abcd".chars().collect();
        assert_eq!(matching_blocks(&a, &b), vec![(0, 0, 2), (3, 2, 2)]);
    }
}
