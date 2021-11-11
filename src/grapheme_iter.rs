use std::io::{BufRead, Read, Result};
use std::str::from_utf8;

#[derive(Debug)]
pub struct GraphemeIter<'a> {
    data: &'a str,
    offsets: &'a [usize],
    curr_grapheme: usize,
    curr_grapheme_back: usize,
    curr_offset: usize,
}

impl<'a> Default for GraphemeIter<'a> {
    fn default() -> Self {
        GraphemeIter {
            data: "",
            offsets: &[],
            curr_grapheme: 0,
            curr_grapheme_back: 0,
            curr_offset: 0,
        }
    }
}

//TODO why not just use their iterator and keep the cool get() logic.
impl<'a> GraphemeIter<'a> {
    pub fn new(data: &'a str, offsets: &'a [usize]) -> Self {
        GraphemeIter {
            data,
            offsets,
            curr_grapheme: 0,
            curr_grapheme_back: offsets.len(),
            curr_offset: 0,
        }
    }

    pub fn new_bytes(bytes: &'a [u8], offsets: &'a [usize]) -> Self {
        if let Ok(data) = from_utf8(bytes) {
            GraphemeIter {
                data,
                offsets,
                curr_grapheme: 0,
                curr_grapheme_back: offsets.len(),
                curr_offset: 0,
            }
        } else {
            GraphemeIter {
                data: "",
                offsets,
                curr_grapheme: 0,
                curr_grapheme_back: 0,
                curr_offset: 0,
            }
        }
    }

    pub fn get(&self, idx: usize) -> Option<&'a str> {
        let mut str = None;
        if idx < self.offsets.len() {
            let start = self.offsets[idx];
            let end;
            if idx + 1 == self.offsets.len() {
                str = Some(&self.data[start..]);
            } else {
                end = self.offsets[idx + 1];
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
        Ok(&self.data.as_bytes()[self.curr_offset..])
    }

    fn consume(&mut self, amt: usize) {
        self.curr_offset += amt;
    }
}

impl<'a> From<GraphemeIter<'a>> for String {
    fn from(g_iter: GraphemeIter) -> Self {
        let mut str = String::with_capacity(g_iter.data.len());
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
        if self.curr_grapheme >= self.offsets.len() {
            str = None
        } else {
            let start = self.offsets[self.curr_grapheme];
            self.curr_grapheme += 1;
            let end;
            if self.curr_grapheme == self.offsets.len() {
                str = Some(&self.data[start..]);
            } else {
                end = self.offsets[self.curr_grapheme];
                str = Some(&self.data[start..end]);
            }
        }
        str
    }
}

impl<'a> DoubleEndedIterator for GraphemeIter<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let str;
        if self.curr_grapheme_back == 0 {
            str = None
        } else {
            let start = self.offsets[self.curr_grapheme_back - 1];
            if self.curr_grapheme_back == self.offsets.len() {
                self.curr_grapheme_back -= 1;
                str = Some(&self.data[start..]);
            } else if self.curr_grapheme_back == 1 {
                let end = self.offsets[self.curr_grapheme_back];
                self.curr_grapheme_back -= 1;
                str = Some(&self.data[..end]);
            } else {
                self.curr_grapheme_back -= 1;
                let end = self.offsets[self.curr_grapheme_back + 1];
                str = Some(&self.data[start..end]);
            }
        }
        str
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iterate() {
        let expected_str = String::from("012ते345");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&expected_str, &offsets);
        let mut actual_str = String::with_capacity(12);
        for f in gs {
            actual_str.push_str(f);
        }
        assert_eq!(expected_str, actual_str);
    }

    #[test]
    fn test_iterate_back() {
        let expected_str = String::from("012ते345");
        let expected_rev_str = String::from("543ते210");
        let offsets: Vec<usize> = vec![0, 1, 2, 3, 9, 10, 11];
        let gs = GraphemeIter::new(&expected_str, &offsets);
        let mut actual_rev_str = String::with_capacity(12);
        for x in gs.rev() {
            actual_rev_str.push_str(x);
        }
        assert_eq!(expected_rev_str, actual_rev_str);
    }
}
