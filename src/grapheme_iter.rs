use std::io::{BufRead, Read, Result};

#[derive(Debug)]
pub struct GraphemeIter<'a> {
    data: &'a str,
    offsets: &'a [usize],
    curr_grapheme: usize,
    curr_grapheme_back: isize,
    curr_offset: Option<usize>,
    min_grapheme: usize,
    max_grapheme: usize,
}

impl<'a> Default for GraphemeIter<'a> {
    fn default() -> Self {
        GraphemeIter {
            data: "",
            offsets: &[],
            curr_grapheme: 0,
            curr_grapheme_back: 0,
            curr_offset: Some(0),
            min_grapheme: 0,
            max_grapheme: 0,
        }
    }
}

impl<'a> GraphemeIter<'a> {
    pub fn new(
        data: &'a str,
        offsets: &'a [usize],
        curr_grapheme: usize,
        max_grapheme: usize,
    ) -> Self {
        GraphemeIter {
            data,
            offsets,
            curr_grapheme,
            curr_grapheme_back: max_grapheme as isize - 1,
            curr_offset: offsets.get(curr_grapheme).copied(),
            min_grapheme: curr_grapheme,
            max_grapheme,
        }
    }

    pub fn slice(&self) -> &'a str {
        let min = if self.min_grapheme == 0 {
            None
        } else {
            Some(self.offsets[self.min_grapheme])
        };
        let max = if self.max_grapheme == self.offsets.len() {
            None
        } else {
            Some(self.offsets[self.max_grapheme])
        };
        match (min, max) {
            (None, None) => self.data,
            (Some(start), None) => &self.data[start..],
            (None, Some(end)) => &self.data[..end],
            (Some(start), Some(end)) => &self.data[start..end],
        }
    }

    pub fn get(&self, idx: usize) -> Option<&'a str> {
        let mut str = None;
        if idx < self.max_grapheme {
            let start = self.offsets[idx];
            if idx + 1 == self.max_grapheme {
                str = Some(&self.data[start..]);
            } else {
                let end = self.offsets[idx + 1];
                str = Some(&self.data[start..end]);
            }
        }
        str
    }
}

impl<'a> Read for GraphemeIter<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let len = buf.len() - 1;
        let mut bytes = self.data.as_bytes()[..len].to_owned();
        buf.swap_with_slice(&mut bytes);
        Ok(bytes.len())
    }
}

impl<'a> BufRead for GraphemeIter<'a> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        if let Some(curr_offset) = self.curr_offset {
            if self.max_grapheme == self.offsets.len() {
                Ok(&self.data.as_bytes()[curr_offset..])
            } else {
                let max_offset = self.offsets[self.max_grapheme];
                Ok(&self.data.as_bytes()[curr_offset..max_offset])
            }
        } else {
            Ok(&[])
        }
    }

    fn consume(&mut self, amt: usize) {
        if let Some(curr_offset) = self.curr_offset {
            self.curr_offset = Some(curr_offset + amt);
        }
    }
}

impl<'a> From<GraphemeIter<'a>> for String {
    fn from(g_iter: GraphemeIter) -> Self {
        let mut str = String::new();
        for g in g_iter {
            str.push_str(g);
        }
        str
    }
}

impl<'a> Iterator for GraphemeIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let str;
        if self.curr_grapheme >= self.max_grapheme {
            //base case we've iterated over the edge.
            str = None
        } else if self.max_grapheme - self.min_grapheme == 1 {
            //special case where the buffer is only 1 long
            let start = self.offsets[self.min_grapheme];
            if self.max_grapheme == self.offsets.len() {
                // take care to slice properly if at proper end of self.data
                str = Some(&self.data[start..]);
            } else {
                let end = self.offsets[self.max_grapheme];
                str = Some(&self.data[start..end]);
            }
            self.curr_grapheme = self.max_grapheme + 1;
        } else {
            let start = self.offsets[self.curr_grapheme];
            self.curr_grapheme += 1;

            if self.curr_grapheme == self.max_grapheme {
                // if cursor is at end of iter
                if self.curr_grapheme == self.offsets.len() {
                    // take care to slice properly if at proper end of self.data
                    str = Some(&self.data[start..]);
                } else {
                    let end = self.offsets[self.max_grapheme];
                    str = Some(&self.data[start..end]);
                }
            } else {
                // somewhere in the middle
                let end = self.offsets[self.curr_grapheme];
                str = Some(&self.data[start..end]);
            }
        }
        str
    }
}

impl<'a> DoubleEndedIterator for GraphemeIter<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let str;
        if self.curr_grapheme_back < self.min_grapheme as isize {
            // base case we've run past the min
            str = None
        } else if self.max_grapheme - self.min_grapheme == 1 {
            // special case where buffer is only 1 long
            let start = self.offsets[self.min_grapheme];
            if self.max_grapheme == self.offsets.len() {
                // take care to slice properly if at proper end of self.data
                str = Some(&self.data[start..]);
            } else {
                let end = self.offsets[self.max_grapheme];
                str = Some(&self.data[start..end]);
            }
            self.curr_grapheme_back = -1;
        } else if self.curr_grapheme_back as usize == self.min_grapheme {
            // we've reached the end
            let start = self.offsets[self.min_grapheme];
            let end = self.offsets[self.min_grapheme + 1];
            self.curr_grapheme_back -= 1;
            str = Some(&self.data[start..end]);
        } else {
            let start = self.offsets[self.curr_grapheme_back as usize];
            if self.curr_grapheme_back as usize == self.max_grapheme - 1 {
                // first iteration
                if self.max_grapheme == self.offsets.len() {
                    // take care to slice properly if at proper end of self.data
                    str = Some(&self.data[start..]);
                } else {
                    let max_char = self.offsets[self.curr_grapheme_back as usize + 1];
                    str = Some(&self.data[start..max_char]);
                }
                self.curr_grapheme_back -= 1;
            } else if self.curr_grapheme_back == self.min_grapheme as isize {
                // last iteration
                let end = self.offsets[self.curr_grapheme_back as usize + 1];
                self.curr_grapheme_back -= 1;
                if self.min_grapheme == 0 {
                    // take care to slice properly if at proper end of self.data
                    str = Some(&self.data[..end]);
                } else {
                    str = Some(&self.data[start..end]);
                }
            } else {
                // somewhere in the middle
                let end = self.offsets[self.curr_grapheme_back as usize + 1];
                self.curr_grapheme_back -= 1;
                str = Some(&self.data[start..end]);
            }
        }
        str
    }
}

impl<'a> ExactSizeIterator for GraphemeIter<'a> {
    fn len(&self) -> usize {
        if self.curr_grapheme_back < 0 {
            0
        } else {
            self.curr_grapheme_back as usize + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get() {
        let base: String = String::from("012\u{924}\u{947}345");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&base, &offsets, 0, 7);
        assert_eq!("5", gs.get(6).unwrap());
        assert_eq!("\u{924}\u{947}", gs.get(3).unwrap());
    }

    #[test]
    fn test_slices() {
        let base: String = String::from("012\u{924}\u{947}345");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let true_min_artificial_max = GraphemeIter::new(&base, &offsets, 0, 6);
        assert_eq!("012\u{924}\u{947}34", true_min_artificial_max.slice());

        let true_min_and_max = GraphemeIter::new(&base, &offsets, 0, 7);
        assert_eq!("012\u{924}\u{947}345", true_min_and_max.slice());

        let artificial_min_and_true_max = GraphemeIter::new(&base, &offsets, 2, 7);
        assert_eq!("2\u{924}\u{947}345", artificial_min_and_true_max.slice());

        let artificial_min_and_max = GraphemeIter::new(&base, &offsets, 2, 4);
        assert_eq!("2\u{924}\u{947}", artificial_min_and_max.slice());

        let one_grapheme = GraphemeIter::new(&base, &offsets, 3, 4);
        assert_eq!("\u{924}\u{947}", one_grapheme.slice());

        let no_graphemes = GraphemeIter::new(&base, &offsets, 3, 3);
        assert_eq!("", no_graphemes.slice());
    }

    #[test]
    fn test_iterate() {
        let expected_str: String = String::from("012\u{924}\u{947}345");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&expected_str, &offsets, 0, 7);
        let gs2 = GraphemeIter::new(&expected_str, &offsets, 0, 7);
        let mut actual_str = String::with_capacity(12);
        for f in gs {
            actual_str.push_str(f);
        }
        assert_eq!(expected_str, actual_str);
        assert_eq!(expected_str, gs2.slice());
    }

    #[test]
    fn test_iterate_forward_slice() {
        let base: String = String::from("012\u{924}\u{947}345");
        let expected_str = String::from("\u{924}\u{947}34");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&base, &offsets, 3, 6);
        let gs2 = GraphemeIter::new(&base, &offsets, 3, 6);
        let mut actual_str = String::with_capacity(8);
        for f in gs {
            actual_str.push_str(f);
        }
        assert_eq!(expected_str, actual_str);
        assert_eq!(expected_str, gs2.slice());
    }

    #[test]
    fn test_iterate_back() {
        let expected_str: String = String::from("012\u{924}\u{947}345");
        let expected_rev_str = String::from("543\u{924}\u{947}210");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&expected_str, &offsets, 0, 7);
        let gs2 = GraphemeIter::new(&expected_str, &offsets, 0, 7);
        let mut actual_rev_str = String::with_capacity(12);
        for x in gs.rev() {
            actual_rev_str.push_str(x);
        }
        assert_eq!(expected_rev_str, actual_rev_str);
        assert_eq!(expected_str, gs2.slice());
    }

    #[test]
    fn test_iterate_backwards_slice() {
        let expected_str: String = String::from("012\u{924}\u{947}345");
        let expected_rev_str = String::from("43\u{924}\u{947}");
        let expected_slice_str = String::from("\u{924}\u{947}34");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&expected_str, &offsets, 3, 6);
        let gs2 = GraphemeIter::new(&expected_str, &offsets, 3, 6);
        let mut actual_rev_str = String::with_capacity(8);
        for x in gs.rev() {
            actual_rev_str.push_str(x);
        }
        assert_eq!(expected_rev_str, actual_rev_str);
        assert_eq!(expected_slice_str, gs2.slice());
    }
}
