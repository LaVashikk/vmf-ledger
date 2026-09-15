//! Pairing blocks between two states of a VMF.
//!
//! Hammer rewrites a map wholesale on save: block order gets shuffled and `id`
//! keyvalues are not guaranteed to survive. Positional or id-only comparison
//! therefore reports half the file as changed, which is why a plain `git diff`
//! over VMF is unreadable.
//!
//! Matching runs as a cascade, cheapest and most certain first. Each stage only
//! sees what the previous ones could not place, so the quadratic stage normally
//! runs over a handful of blocks:
//!
//! 1. **Marker** - a key the tool itself wrote. Exact, O(1).
//! 2. **Id** - the `id` keyvalue. Exact, O(1), but Hammer may renumber.
//! 3. **Signature** - the discriminating keyvalues taken together. Exact, O(1),
//!    and only trusted when the signature is unique on both sides.
//! 4. **Similarity** - weighted Jaccard over every keyvalue. O(k^2) per bucket.

use std::collections::HashMap;

use source_vmf::VmfBlock;

/// Keyvalues that carry most of a block's identity, used by the signature stage.
pub const DEFAULT_SIGNATURE_KEYS: &[&str] = &["classname", "targetname", "origin"];

/// Stage that produced a matched pair, ordered by confidence level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Similarity,
    Signature,
    Id,
    Marker,
}

/// One matched pair of blocks, as indices into the slices handed to
/// [`match_blocks`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    pub old: usize,
    pub new: usize,
    pub confidence: Confidence,
    /// Jaccard score for [`Confidence::Similarity`], `1.0` for the exact stages.
    pub score: f32,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Matching {
    pub pairs: Vec<Pair>,
    /// Indices with no counterpart: removed, if `old` is the earlier state.
    pub only_old: Vec<usize>,
    /// Indices with no counterpart: added.
    pub only_new: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct MatchOptions<'a> {
    /// Key holding an entity marker token. Evaluated first; `None` skips this stage.
    pub marker_key: Option<&'a str>,
    pub signature_keys: &'a [&'a str],
    /// Keys excluded from similarity scoring.
    ///
    /// Unique identifiers distort rarity weighting when exact matching fails,
    /// artificially deflating Jaccard similarity scores.
    pub ignore_keys: &'a [&'a str],
    /// Minimum similarity score required to accept a match.
    pub min_score: f32,
}

impl Default for MatchOptions<'_> {
    fn default() -> Self {
        Self {
            marker_key: None,
            signature_keys: DEFAULT_SIGNATURE_KEYS,
            ignore_keys: &["id"],
            min_score: 0.5,
        }
    }
}

/// Matches blocks between `old` and `new` across cascade stages.
///
/// Blocks are partitioned by [`VmfBlock::name`]; matching never crosses different block types.
pub fn match_blocks(old: &[VmfBlock], new: &[VmfBlock], opts: &MatchOptions<'_>) -> Matching {
    let mut state = State::new(old, new);

    for group in name_groups(old, new) {
        if let Some(key) = opts.marker_key {
            state.pair_on(&group, Confidence::Marker, |b| kv(b, key));
        }
        state.pair_on(&group, Confidence::Id, |b| kv(b, "id"));
        state.pair_on(&group, Confidence::Signature, |b| {
            signature(b, opts.signature_keys)
        });
        let mut ignored: Vec<&str> = opts.ignore_keys.to_vec();
        ignored.extend(opts.marker_key);
        state.pair_by_similarity(&group, opts.min_score, &ignored);
    }

    state.finish()
}

/// Indices of one block name on both sides. Matching never crosses a group.
struct Group {
    old: Vec<usize>,
    new: Vec<usize>,
}

fn name_groups(old: &[VmfBlock], new: &[VmfBlock]) -> Vec<Group> {
    let mut groups: HashMap<&str, Group> = HashMap::new();
    for (i, block) in old.iter().enumerate() {
        groups
            .entry(block.name.as_str())
            .or_insert_with(|| Group {
                old: Vec::new(),
                new: Vec::new(),
            })
            .old
            .push(i);
    }
    for (i, block) in new.iter().enumerate() {
        groups
            .entry(block.name.as_str())
            .or_insert_with(|| Group {
                old: Vec::new(),
                new: Vec::new(),
            })
            .new
            .push(i);
    }
    groups.into_values().collect()
}

fn kv(block: &VmfBlock, key: &str) -> Option<String> {
    block.key_values.get(key).cloned()
}

// Returns joined keyvalue pairs for configured signature keys present on the block.
fn signature(block: &VmfBlock, keys: &[&str]) -> Option<String> {
    let mut parts = Vec::new();
    for key in keys {
        if let Some(value) = block.key_values.get(*key) {
            parts.push(format!("{key}={value}"));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\u{1}"))
}

struct State<'a> {
    old: &'a [VmfBlock],
    new: &'a [VmfBlock],
    old_taken: Vec<bool>,
    new_taken: Vec<bool>,
    pairs: Vec<Pair>,
}

impl<'a> State<'a> {
    fn new(old: &'a [VmfBlock], new: &'a [VmfBlock]) -> Self {
        Self {
            old,
            new,
            old_taken: vec![false; old.len()],
            new_taken: vec![false; new.len()],
            pairs: Vec::new(),
        }
    }

    // Pairs blocks whose derived token is identical and unique on both sides.
    // Ambiguous tokens occurring multiple times on either side are deferred to later stages.
    fn pair_on(
        &mut self,
        group: &Group,
        confidence: Confidence,
        token: impl Fn(&VmfBlock) -> Option<String>,
    ) {
        let mut old_by_token: HashMap<String, Vec<usize>> = HashMap::new();
        for &i in &group.old {
            if self.old_taken[i] {
                continue;
            }
            if let Some(t) = token(&self.old[i]) {
                old_by_token.entry(t).or_default().push(i);
            }
        }

        let mut new_by_token: HashMap<String, Vec<usize>> = HashMap::new();
        for &j in &group.new {
            if self.new_taken[j] {
                continue;
            }
            if let Some(t) = token(&self.new[j]) {
                new_by_token.entry(t).or_default().push(j);
            }
        }

        for (t, olds) in old_by_token {
            let Some(news) = new_by_token.get(&t) else {
                continue;
            };
            if olds.len() != 1 || news.len() != 1 {
                continue;
            }
            self.take(olds[0], news[0], confidence, 1.0);
        }
    }

    fn pair_by_similarity(&mut self, group: &Group, min_score: f32, ignored: &[&str]) {
        let olds: Vec<usize> = group
            .old
            .iter()
            .copied()
            .filter(|&i| !self.old_taken[i])
            .collect();
        let news: Vec<usize> = group
            .new
            .iter()
            .copied()
            .filter(|&j| !self.new_taken[j])
            .collect();
        if olds.is_empty() || news.is_empty() {
            return;
        }

        let weights = Weights::build(
            olds.iter().map(|&i| &self.old[i]),
            news.iter().map(|&j| &self.new[j]),
            ignored,
        );

        // Compute pairwise similarity scores above the threshold.
        let mut scored: Vec<(f32, usize, usize)> = olds
            .iter()
            .flat_map(|&i| news.iter().map(move |&j| (i, j)))
            .map(|(i, j)| (weights.score(&self.old[i], &self.new[j]), i, j))
            .filter(|&(score, ..)| score >= min_score)
            .collect();

        scored.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (score, i, j) in scored {
            if self.old_taken[i] || self.new_taken[j] {
                continue;
            }
            self.take(i, j, Confidence::Similarity, score);
        }
    }

    fn take(&mut self, old: usize, new: usize, confidence: Confidence, score: f32) {
        self.old_taken[old] = true;
        self.new_taken[new] = true;
        self.pairs.push(Pair {
            old,
            new,
            confidence,
            score,
        });
    }

    fn finish(mut self) -> Matching {
        self.pairs.sort_unstable_by_key(|p| (p.old, p.new));
        Matching {
            only_old: (0..self.old.len())
                .filter(|&i| !self.old_taken[i])
                .collect(),
            only_new: (0..self.new.len())
                .filter(|&j| !self.new_taken[j])
                .collect(),
            pairs: self.pairs,
        }
    }
}

// Inverse document frequency weighting over keyvalue pairs.
struct Weights<'a> {
    df: HashMap<(&'a str, &'a str), u32>,
    ignored: &'a [&'a str],
}

impl<'a> Weights<'a> {
    fn build(
        old: impl Iterator<Item = &'a VmfBlock>,
        new: impl Iterator<Item = &'a VmfBlock>,
        ignored: &'a [&'a str],
    ) -> Self {
        let mut df: HashMap<(&str, &str), u32> = HashMap::new();
        for block in old.chain(new) {
            for (k, v) in &block.key_values {
                if ignored.contains(&k.as_str()) {
                    continue;
                }
                *df.entry((k.as_str(), v.as_str())).or_insert(0) += 1;
            }
        }
        Self { df, ignored }
    }

    fn weight(&self, k: &str, v: &str) -> f32 {
        // Unseen keyvalue pairs default to count 1 (maximum weight).
        let df = self.df.get(&(k, v)).copied().unwrap_or(1);
        1.0 / df as f32
    }

    /// Weighted Jaccard: shared weight over union weight, in `0.0..=1.0`.
    fn score(&self, a: &VmfBlock, b: &VmfBlock) -> f32 {
        let mut shared = 0.0;
        let mut union = 0.0;

        for (k, v) in &a.key_values {
            if self.ignored.contains(&k.as_str()) {
                continue;
            }
            let w = self.weight(k, v);
            union += w;
            if b.key_values.get(k).is_some_and(|other| other == v) {
                shared += w;
            }
        }
        for (k, v) in &b.key_values {
            if self.ignored.contains(&k.as_str()) {
                continue;
            }
            if a.key_values.get(k).is_none_or(|other| other != v) {
                union += self.weight(k, v);
            }
        }

        if union == 0.0 { 0.0 } else { shared / union }
    }
}
