//! Register values of FVP_Corstone_SSE-320 at reset, dumped through its GDB server
//! (`fixtures/fvp/regs-reset.txt`, made with `tools/fvp_dump.py`). They serve as the reset
//! values of the register blocks that are not modelled functionally (KMU, LCM, SAM, PPUs, …)
//! and as the identification registers of all blocks.

use std::sync::OnceLock;

static DUMP: OnceLock<Vec<(u32, u32)>> = OnceLock::new();

fn table() -> &'static [(u32, u32)] {
    DUMP.get_or_init(|| {
        let mut v: Vec<(u32, u32)> = include_str!("../../../fixtures/fvp/regs-reset.txt")
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let a = it.next()?.strip_prefix("0x")?;
                let w = it.next()?.strip_prefix("0x")?;
                Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(w, 16).ok()?))
            })
            .collect();
        v.sort_by_key(|(a, _)| *a);
        v.dedup_by_key(|(a, _)| *a);
        v
    })
}

/// The word the FVP read at `addr` (any alias) at reset, if it was dumped.
pub fn word(addr: u32) -> Option<u32> {
    let t = table();
    let find = |a: u32| t.binary_search_by_key(&a, |(x, _)| *x).ok().map(|i| t[i].1);
    find(addr).or_else(|| find(addr ^ 0x1000_0000))
}
