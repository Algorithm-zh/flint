use std::cmp::Ordering;
use std::sync::Arc;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::model::{Entry, Hit, Seg};

/// Immutable, search-ready snapshot of the index.
///
/// `entries` is shared by `Arc` so that updating the frequency table (which
/// happens on every launch) never has to copy the corpus.
pub struct Index {
    pub entries: Arc<Vec<Entry>>,
    freq: Arc<FxHashMap<String, u32>>,
}

#[inline]
fn freq_bonus(count: u32) -> i64 {
    match count {
        0 => 0,
        n => {
            // log-ish curve: the first few launches matter a lot, the rest taper off.
            let f = (n as f64 + 1.0).ln() * 340.0;
            f as i64
        }
    }
}

impl Index {
    pub fn new(entries: Arc<Vec<Entry>>, freq: FxHashMap<String, u32>) -> Self {
        Self {
            entries,
            freq: Arc::new(freq),
        }
    }

    /// Same corpus, refreshed frequency table.
    pub fn with_freq(&self, freq: FxHashMap<String, u32>) -> Self {
        Self {
            entries: self.entries.clone(),
            freq: Arc::new(freq),
        }
    }

    pub fn empty() -> Self {
        Self {
            entries: Arc::new(Vec::new()),
            freq: Arc::new(FxHashMap::default()),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Score-and-rank. Returns at most `limit` hits.
    ///
    /// Two passes: the first skips index collection (cheap, parallel over the
    /// whole corpus), the second re-runs the matcher on the winners only to
    /// recover highlight positions.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let q = query.trim();
        if q.is_empty() {
            return self.top_used(limit);
        }

        let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);

        let n = self.entries.len();
        if n == 0 {
            return Vec::new();
        }

        let chunk = (n / (rayon::current_num_threads() * 4)).max(1024);

        let mut cfg = Config::DEFAULT.match_paths();
        // A name that starts with the query is almost always what the user
        // means; without this, "chrome" ranks a long file containing
        // c…h…r…o…m…e somewhere in the middle as a near-equal hit.
        cfg.prefer_prefix = true;

        let mut scored: Vec<(i64, u32)> = self
            .entries
            .par_chunks(chunk)
            .enumerate()
            .flat_map(|(ci, slice)| {
                let mut matcher = Matcher::new(cfg.clone());
                let mut buf: Vec<char> = Vec::with_capacity(64);
                let mut buf2: Vec<char> = Vec::with_capacity(64);
                let mut local: Vec<(i64, u32)> = Vec::with_capacity(64);
                let base = (ci * chunk) as u32;

                for (i, e) in slice.iter().enumerate() {
                    let hay = Utf32Str::new(&e.name, &mut buf);
                    let mut sc = pattern.score(hay, &mut matcher);

                    // Fall back to the hidden alias (pinyin, English package id,
                    // common shorthand). Only apps carry one, so this is a tiny
                    // fraction of the corpus.
                    if sc.is_none() && !e.alias.is_empty() {
                        let hay2 = Utf32Str::new(&e.alias, &mut buf2);
                        if let Some(s2) = pattern.score(hay2, &mut matcher) {
                            sc = Some(s2);
                        }
                    }

                    if let Some(s) = sc {
                        // An alias is an exact synonym, not a fuzzy guess — no
                        // discount. "vscode" meaning "Visual Studio Code" should
                        // score like a real hit.
                        let bonus = match self.freq.get(&e.path) {
                            Some(&c) => freq_bonus(c),
                            None => 0,
                        };
                        local.push((s as i64 * 100 + bonus + e.kind.bias(), base + i as u32));
                    }
                }
                local
            })
            .collect();

        if scored.is_empty() {
            return Vec::new();
        }

        // Fuzzy matching is generous: typing "chrome" also matches any file whose
        // name happens to contain c…h…r…o…m…e in order. Keep a wider candidate
        // pool, then drop everything that scores far below the best hit.
        let pool = (limit * 5).min(scored.len());
        if scored.len() > pool {
            scored.select_nth_unstable_by(pool - 1, |a, b| b.0.cmp(&a.0));
            scored.truncate(pool);
        }
        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let best = scored[0].0;
        if best > 0 {
            // 55% of the winner: keeps genuine runners-up (typing "apifox"
            // should still offer "Apipost") while cutting scattered-character
            // noise like V65FIX_MOCHA_ON_V63_… "matching" chrome.
            let floor = best * 55 / 100;
            scored.retain(|(s, _)| *s >= floor);
        }
        scored.truncate(limit);

        let mut matcher = Matcher::new(cfg);
        let mut idxbuf: Vec<u32> = Vec::with_capacity(32);
        let mut chars: Vec<char> = Vec::with_capacity(64);

        scored
            .into_iter()
            .map(|(_, i)| {
                let e = &self.entries[i as usize];
                let hay = Utf32Str::new(&e.name, &mut chars);
                idxbuf.clear();
                // Alias-only matches leave the name unhighlighted, which is correct:
                // there is nothing visible to point at.
                if pattern.indices(hay, &mut matcher, &mut idxbuf).is_none() {
                    idxbuf.clear();
                }
                let segs = split_segments(&e.name, &idxbuf);
                Hit {
                    path: e.path.clone(),
                    name: e.name.clone(),
                    ext: e.ext.clone(),
                    kind: e.kind.as_str(),
                    hint: e.hint.clone(),
                    icon_src: e.icon_src.clone(),
                    segs,
                }
            })
            .collect()
    }

    /// Empty-query state: whatever the user reaches for most often.
    fn top_used(&self, limit: usize) -> Vec<Hit> {
        let mut ranked: Vec<(i64, &Entry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let c = *self.freq.get(&e.path)?;
                Some((freq_bonus(c) + e.kind.bias(), e))
            })
            .collect();

        if ranked.is_empty() {
            // Nothing has been launched yet: show a tidy alphabetical slice of
            // the apps rather than whatever order the index happened to be in.
            let mut apps: Vec<&Entry> = self
                .entries
                .iter()
                .filter(|e| e.kind == crate::model::Kind::App)
                .collect();
            apps.sort_unstable_by(|a, b| a.name.cmp(&b.name));
            return apps
                .into_iter()
                .take(limit)
                .map(|e| Hit {
                    path: e.path.clone(),
                    name: e.name.clone(),
                    ext: e.ext.clone(),
                    kind: e.kind.as_str(),
                    hint: e.hint.clone(),
                    icon_src: e.icon_src.clone(),
                    segs: vec![Seg {
                        t: e.name.clone(),
                        h: false,
                    }],
                })
                .collect();
        }

        ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        ranked
            .into_iter()
            .take(limit)
            .map(|(_, e)| Hit {
                path: e.path.clone(),
                name: e.name.clone(),
                ext: e.ext.clone(),
                kind: e.kind.as_str(),
                hint: e.hint.clone(),
                icon_src: e.icon_src.clone(),
                segs: vec![Seg {
                    t: e.name.clone(),
                    h: false,
                }],
            })
            .collect()
    }
}

/// Search the live window list.
///
/// Kept separate from `Index::search` because windows change between summons:
/// they're enumerated fresh on every show, not baked into the snapshot.
pub fn search_windows(query: &str, wins: &[crate::windows_list::WinEntry], limit: usize) -> Vec<Hit> {
    let q = query.trim();
    if q.is_empty() || wins.is_empty() {
        return Vec::new();
    }

    let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf: Vec<char> = Vec::with_capacity(64);
    let mut buf2: Vec<char> = Vec::with_capacity(64);
    let mut idxbuf: Vec<u32> = Vec::with_capacity(32);

    let mut scored: Vec<(u32, usize)> = Vec::with_capacity(wins.len());
    for (i, w) in wins.iter().enumerate() {
        let hay = Utf32Str::new(&w.title, &mut buf);
        let mut s = pattern.score(hay, &mut matcher);
        if s.is_none() && !w.exe.is_empty() {
            let hay2 = Utf32Str::new(&w.exe, &mut buf2);
            s = pattern.score(hay2, &mut matcher);
        }
        if let Some(s) = s {
            scored.push((s, i));
        }
    }

    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(limit);

    scored
        .into_iter()
        .map(|(_, i)| {
            let w = &wins[i];
            let hay = Utf32Str::new(&w.title, &mut buf);
            idxbuf.clear();
            if pattern.indices(hay, &mut matcher, &mut idxbuf).is_none() {
                idxbuf.clear();
            }
            Hit {
                path: format!("hwnd:{}", w.hwnd),
                name: w.title.clone(),
                ext: "WIN".into(),
                kind: "window",
                hint: w.exe.clone(),
                icon_src: w.exe_path.clone(),
                segs: split_segments(&w.title, &idxbuf),
            }
        })
        .collect()
}

/// The open windows as hits, unfiltered. This is what the palette shows when
/// nothing has been typed yet: the most direct answer to "what can I switch to
/// right now" is the list of what is already on screen.
pub fn windows_as_hits(wins: &[crate::windows_list::WinEntry], limit: usize) -> Vec<Hit> {
    wins.iter()
        .take(limit)
        .map(|w| Hit {
            path: format!("hwnd:{}", w.hwnd),
            name: w.title.clone(),
            ext: "WIN".into(),
            kind: "window",
            hint: w.exe.clone(),
            icon_src: w.exe_path.clone(),
            segs: vec![Seg {
                t: w.title.clone(),
                h: false,
            }],
        })
        .collect()
}

/// Turn `(name, match positions)` into alternating plain/highlight runs.
fn split_segments(name: &str, positions: &[u32]) -> Vec<Seg> {
    if positions.is_empty() {
        return vec![Seg {
            t: name.to_string(),
            h: false,
        }];
    }

    let mut sorted: Vec<u32> = positions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut segs: Vec<Seg> = Vec::with_capacity(sorted.len() * 2 + 1);
    let mut plain = String::new();
    let mut hl = String::new();
    let mut next = 0usize;

    for (ci, ch) in name.chars().enumerate() {
        let is_hit = sorted.get(next).map(|&p| p as usize) == Some(ci);
        if is_hit {
            next += 1;
            if !plain.is_empty() {
                segs.push(Seg {
                    t: std::mem::take(&mut plain),
                    h: false,
                });
            }
            hl.push(ch);
        } else {
            if !hl.is_empty() {
                segs.push(Seg {
                    t: std::mem::take(&mut hl),
                    h: true,
                });
            }
            plain.push(ch);
        }
    }
    if !plain.is_empty() {
        segs.push(Seg { t: plain, h: false });
    }
    if !hl.is_empty() {
        segs.push(Seg { t: hl, h: true });
    }
    segs
}

#[allow(dead_code)]
fn _cmp(a: &(i64, u32), b: &(i64, u32)) -> Ordering {
    b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1))
}
