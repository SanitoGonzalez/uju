use std::cell::Cell;

/// A bit per dense position of a table, cleared every tick.
///
/// Its words always cover the table's length, so a [`Marker`] can set any
/// entry's bit without growing them.
#[derive(Default)]
pub(crate) struct Bits {
    words: Vec<u64>,
}

impl Bits {
    /// Grows the words to cover `len` positions. New bits are clear.
    #[inline]
    pub(crate) fn cover(&mut self, len: usize) {
        let words = len.div_ceil(64);
        if words > self.words.len() {
            self.words.resize(words, 0);
        }
    }

    #[inline]
    pub(crate) fn get(&self, position: usize) -> bool {
        self.words
            .get(position / 64)
            .is_some_and(|word| word & (1 << (position % 64)) != 0)
    }

    /// Sets the bit at `position`, which the words must cover.
    #[inline]
    pub(crate) fn set(&mut self, position: usize) {
        self.words[position / 64] |= 1 << (position % 64);
    }

    /// Moves the bit at `last` to `position` and clears `last`, as a
    /// `swap_remove` moves an entry.
    #[inline]
    pub(crate) fn swap_remove(&mut self, position: usize, last: usize) {
        let bit = self.get(last);
        self.assign(position, bit);
        self.assign(last, false);
    }

    #[inline]
    fn assign(&mut self, position: usize, bit: bool) {
        let mask = 1 << (position % 64);
        let word = &mut self.words[position / 64];
        if bit {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }

    /// Clears every bit.
    pub(crate) fn clear(&mut self) {
        self.words.fill(0);
    }

    /// The positions whose bit is set, in ascending order.
    pub(crate) fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(index, &word)| {
            let mut word = word;
            std::iter::from_fn(move || {
                if word == 0 {
                    return None;
                }
                let bit = word.trailing_zeros() as usize;
                word &= word - 1;
                Some(index * 64 + bit)
            })
        })
    }

    /// A handle that sets bits through a shared borrow, so the marking
    /// wrappers alive at once can share words.
    #[inline]
    pub(crate) fn marker(&mut self) -> Marker<'_> {
        Marker(Cell::from_mut(self.words.as_mut_slice()).as_slice_of_cells())
    }
}

/// Sets [`Bits`] through `&Cell<u64>`: a plain load and store, as a `World`
/// runs on one thread.
#[derive(Clone, Copy)]
pub(crate) struct Marker<'a>(&'a [Cell<u64>]);

impl Marker<'_> {
    #[inline]
    pub(crate) fn set(self, position: usize) {
        let word = &self.0[position / 64];
        word.set(word.get() | 1 << (position % 64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_swap_remove() {
        let mut bits = Bits::default();
        bits.cover(130);
        assert_eq!(bits.words.len(), 3);
        bits.set(3);
        bits.set(129);
        bits.marker().set(64);
        assert_eq!(bits.ones().collect::<Vec<_>>(), [3, 64, 129]);

        // the last entry's bit moves into the hole
        bits.swap_remove(5, 129);
        assert_eq!(bits.ones().collect::<Vec<_>>(), [3, 5, 64]);
        bits.swap_remove(3, 128);
        assert_eq!(bits.ones().collect::<Vec<_>>(), [5, 64]);
        bits.swap_remove(64, 64);
        assert_eq!(bits.ones().collect::<Vec<_>>(), [5]);

        bits.clear();
        assert_eq!(bits.ones().count(), 0);
        assert!(!bits.get(5));
        assert!(!bits.get(1000));
    }
}
