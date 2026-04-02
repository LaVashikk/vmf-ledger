//! Pairing blocks between two states of a VMF.
//!
//! Hammer rewrites a map wholesale on save: block order gets shuffled and `id`
//! keyvalues are not guaranteed to survive. Positional comparison therefore
//! reports half the file as changed, which is why a plain `git diff` over VMF
//! is unreadable.

use std::collections::HashMap;

use vmf_forge::VmfBlock;

/// One matched pair of blocks, as indices into the slices handed to
/// [`match_blocks`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    pub old: usize,
    pub new: usize,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Matching {
    pub pairs: Vec<Pair>,
    /// Indices with no counterpart: removed, if `old` is the earlier state.
    pub only_old: Vec<usize>,
    /// Indices with no counterpart: added.
    pub only_new: Vec<usize>,
}

/// Pairs `old` against `new` on the `id` keyvalue.
///
/// Blocks are only ever paired with blocks of the same [`VmfBlock::name`], so a
/// `solid` can never be mistaken for an `entity` however similar their keys are.
pub fn match_blocks(old: &[VmfBlock], new: &[VmfBlock]) -> Matching {
    let mut state = State::new(old, new);

    for group in name_groups(old, new) {
        state.pair_on(&group, |b| kv(b, "id"));
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

    /// Pairs blocks whose `token` is equal and unique on both sides.
    ///
    /// Ambiguity is deliberately left alone instead of resolved by picking the
    /// first hit: two blocks sharing an id after a copy-paste are a real case,
    /// and guessing there is how a rollback corrupts a map.
    fn pair_on(&mut self, group: &Group, token: impl Fn(&VmfBlock) -> Option<String>) {
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
            self.take(olds[0], news[0]);
        }
    }

    fn take(&mut self, old: usize, new: usize) {
        self.old_taken[old] = true;
        self.new_taken[new] = true;
        self.pairs.push(Pair { old, new });
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
