//! Tryte-addressed memory (AM §2, TRISC-27 §2.1).
//!
//! Storage is allocated sparsely, one page at a time. A program that sets its
//! stack pointer to the top of memory and its code at the bottom touches two
//! pages, not the whole address space, so a machine with a large `A` costs
//! nothing until it is used.

use crate::word::{MAX_TRYTE, WORD_TRYTES};
use std::collections::BTreeMap;

/// Trytes per page — 3⁹, one tryte's worth of trytes.
const PAGE: i128 = 19683;

/// A sparse tryte array of a fixed size.
pub struct Memory {
    size: i128,
    pages: BTreeMap<i128, Box<[i16]>>,
}

impl Memory {
    /// A zero-filled address space of `size` trytes.
    pub fn new(size: i128) -> Memory {
        Memory {
            size,
            pages: BTreeMap::new(),
        }
    }

    /// The address-space size, A.
    pub fn size(&self) -> i128 {
        self.size
    }

    /// True iff `addr` is a memory address of this machine.
    pub fn contains(&self, addr: i128) -> bool {
        0 <= addr && addr < self.size
    }

    /// The tryte at `addr`, which must be in range.
    pub fn tryte(&self, addr: i128) -> i128 {
        let (page, off) = split(addr);
        match self.pages.get(&page) {
            Some(p) => p[off] as i128,
            None => 0,
        }
    }

    /// Write the tryte at `addr`, which must be in range.
    pub fn set_tryte(&mut self, addr: i128, v: i128) {
        debug_assert!(
            (-MAX_TRYTE..=MAX_TRYTE).contains(&v),
            "{v} is not a tryte value"
        );
        let (page, off) = split(addr);
        let p = self
            .pages
            .entry(page)
            .or_insert_with(|| vec![0i16; PAGE as usize].into_boxed_slice());
        p[off] = v as i16;
    }

    /// The word at `addr`: three trytes, least significant at the lowest
    /// address (AM §2.2).
    pub fn word(&self, addr: i128) -> i128 {
        (0..WORD_TRYTES)
            .map(|i| self.tryte(addr + i) * 3i128.pow(9 * i as u32))
            .sum()
    }

    /// Write the word at `addr`, little-trytean.
    pub fn set_word(&mut self, addr: i128, v: i128) {
        for (i, t) in crate::word::word_trytes(v).into_iter().enumerate() {
            self.set_tryte(addr + i as i128, t as i128);
        }
    }

    /// Load an image at address 0, one tryte per element.
    pub fn load_image(&mut self, trytes: &[i16]) {
        for (i, &t) in trytes.iter().enumerate() {
            self.set_tryte(i as i128, t as i128);
        }
    }
}

fn split(addr: i128) -> (i128, usize) {
    (addr.div_euclid(PAGE), addr.rem_euclid(PAGE) as usize)
}
