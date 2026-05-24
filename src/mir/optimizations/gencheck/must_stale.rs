//! Definite-staleness lattice: a second, independent forward analysis
//! alongside `solve`'s may-analysis. Where `solve` proves a use only *might*
//! read a freed value (and inserts a runtime check), this proves a use
//! *always* reads one, on every path, with no owning redefinition of any
//! root since -- that is a compile error (E0708), not a check.
//!
//! Suspects with an `unknown` owner never qualify: a root fully known is the
//! whole premise this analysis needs. Same for suspects with more than 64
//! roots (bitmask capacity) or none at all (nothing to prove against).

use super::SuspectInfo;
use crate::mir::*;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

const MAX_ROOTS: usize = 64;

/// One eligible suspect's root set, indexed into a `u64` bitmask: bit `i` set
/// means `root_list[i]` has definitely been dropped, on every path reaching
/// this program point, with no owning redefinition since.
struct MustInfo {
    suspects: Vec<Local>,
    index_of: HashMap<Local, usize>,
    full_mask: Vec<u64>,
    /// Root local -> every (suspect index, bit index) it backs.
    reverse: HashMap<Local, Vec<(usize, usize)>>,
}

fn full_mask_for(len: usize) -> u64 {
    if len >= 64 {
        u64::MAX
    } else {
        (1u64 << len) - 1
    }
}

fn build_must_info(info: &[SuspectInfo], checkable: &[bool]) -> MustInfo {
    let mut suspects = Vec::new();
    let mut root_list = Vec::new();
    for (i, s) in info.iter().enumerate() {
        if i != 0
            && s.suspect
            && !s.unknown
            && checkable.get(i).copied().unwrap_or(false)
            && !s.roots.is_empty()
            && s.roots.len() <= MAX_ROOTS
        {
            let mut roots: Vec<Local> = s.roots.iter().copied().collect();
            roots.sort_unstable_by_key(|l| l.0);
            suspects.push(Local(i));
            root_list.push(roots);
        }
    }
    let index_of: HashMap<Local, usize> =
        suspects.iter().enumerate().map(|(i, &l)| (l, i)).collect();
    let full_mask: Vec<u64> = root_list.iter().map(|r| full_mask_for(r.len())).collect();

    let mut reverse: HashMap<Local, Vec<(usize, usize)>> = HashMap::default();
    for (s, roots) in root_list.iter().enumerate() {
        for (bit, &r) in roots.iter().enumerate() {
            reverse.entry(r).or_default().push((s, bit));
        }
    }

    MustInfo {
        suspects,
        index_of,
        full_mask,
        reverse,
    }
}

/// A use that read a value already fully proven stale. `free_span` is the
/// most recent qualifying drop seen along the way to this use -- with
/// multiple roots, whichever one this path last observed being dropped;
/// exact provenance doesn't affect the u64 mask that decides the verdict,
/// only which line the diagnostic's secondary label points at.
pub(super) struct MustStaleUse {
    pub bb: usize,
    pub idx: usize,
    pub local: Local,
    pub free_span: Span,
}

pub(super) fn find_must_stale(
    func: &MirFunction,
    info: &[SuspectInfo],
    checkable: &[bool],
) -> Vec<MustStaleUse> {
    let must = build_must_info(info, checkable);
    if must.suspects.is_empty() {
        return Vec::new();
    }
    let k = must.suspects.len();
    let nb = func.basic_blocks.len();
    if nb == 0 {
        return Vec::new();
    }
    let preds = super::predecessors(func);

    let mut out_mask: Vec<Vec<u64>> = vec![vec![0u64; k]; nb];
    for bb in 0..nb {
        if !preds[bb].is_empty() {
            out_mask[bb] = must.full_mask.clone();
        }
    }
    let mut out_span: Vec<Vec<Option<Span>>> = vec![vec![None; k]; nb];

    let mut changed = true;
    while changed {
        changed = false;
        for bb in 0..nb {
            let (mut mask, mut span) = merge_preds(bb, &preds, &out_mask, &out_span, k);
            transfer_block(func, bb, &must, &mut mask, &mut span, None);
            if mask != out_mask[bb] {
                out_mask[bb] = mask;
                changed = true;
            }
            out_span[bb] = span;
        }
    }

    let mut hits = Vec::new();
    for bb in 0..nb {
        let (mut mask, mut span) = merge_preds(bb, &preds, &out_mask, &out_span, k);
        transfer_block(func, bb, &must, &mut mask, &mut span, Some(&mut hits));
    }
    hits
}

fn merge_preds(
    bb: usize,
    preds: &[Vec<BasicBlockId>],
    out_mask: &[Vec<u64>],
    out_span: &[Vec<Option<Span>>],
    k: usize,
) -> (Vec<u64>, Vec<Option<Span>>) {
    if preds[bb].is_empty() {
        return (vec![0u64; k], vec![None; k]);
    }
    let mut mask = out_mask[preds[bb][0].0].clone();
    let mut span = out_span[preds[bb][0].0].clone();
    for p in preds[bb].iter().skip(1) {
        for i in 0..k {
            mask[i] &= out_mask[p.0][i];
            if span[i].is_none() {
                span[i] = out_span[p.0][i];
            }
        }
    }
    (mask, span)
}

#[allow(clippy::too_many_arguments)]
fn transfer_block(
    func: &MirFunction,
    bb: usize,
    must: &MustInfo,
    masks: &mut [u64],
    spans: &mut [Option<Span>],
    mut hits: Option<&mut Vec<MustStaleUse>>,
) {
    let block = &func.basic_blocks[bb];
    for (idx, stmt) in block.statements.iter().enumerate() {
        // `dst = Move(src)` is itself the hand-off, not a read of stale data:
        // `src`'s bit for root `dst` (registered so some OTHER alias of `src`
        // knows to distrust it once `dst` drops) must not count against `src`
        // at this exact statement, or every ordinary "drop old, move new in"
        // reassignment would falsely read as giving `src` away to itself.
        let self_move_exempt = match &stmt.kind {
            StatementKind::Assign(dst, Rvalue::Use(Operand::Move(src)))
                if must.index_of.contains_key(src) =>
            {
                let s = must.index_of[src];
                must.reverse
                    .get(dst)
                    .into_iter()
                    .flatten()
                    .find(|&&(si, _)| si == s)
                    .map(|&(_, bit)| (s, 1u64 << bit))
            }
            _ => None,
        };

        for l in super::stmt_uses(stmt) {
            if let Some(&s) = must.index_of.get(&l) {
                let effective = match self_move_exempt {
                    Some((es, bit)) if es == s => masks[s] & !bit,
                    _ => masks[s],
                };
                if effective == must.full_mask[s]
                    && let Some(hits) = hits.as_mut()
                {
                    hits.push(MustStaleUse {
                        bb,
                        idx,
                        local: l,
                        free_span: spans[s].unwrap_or(stmt.span),
                    });
                }
            }
        }

        match &stmt.kind {
            StatementKind::Assign(dst, _) => {
                if let Some(&s) = must.index_of.get(dst) {
                    masks[s] = 0;
                    spans[s] = None;
                }
                if let Some(bits) = must.reverse.get(dst) {
                    for &(s, bit) in bits {
                        masks[s] &= !(1u64 << bit);
                        if masks[s] == 0 {
                            spans[s] = None;
                        }
                    }
                }
            }
            StatementKind::Drop(dropped) => {
                if let Some(bits) = must.reverse.get(dropped) {
                    for &(s, bit) in bits {
                        masks[s] |= 1u64 << bit;
                        spans[s] = Some(stmt.span);
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(Terminator {
        kind: TerminatorKind::SwitchInt { discr, .. },
        span,
        ..
    }) = &block.terminator
        && let Operand::Copy(l) | Operand::Move(l) = discr
        && let Some(&s) = must.index_of.get(l)
        && masks[s] == must.full_mask[s]
        && let Some(hits) = hits.as_mut()
    {
        hits.push(MustStaleUse {
            bb,
            idx: usize::MAX,
            local: *l,
            free_span: spans[s].unwrap_or(*span),
        });
    }
}
