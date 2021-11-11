use std::str::from_utf8;

pub struct GraphemeIter<'a> {
    data: &'a str,
    offsets: &'a [usize],
    curr_grapheme: usize,
    curr_grapheme_back: usize,
}

impl<'a> Default for GraphemeIter<'a> {
    fn default() -> Self {
        GraphemeIter {
            data: "",
            offsets: &[],
            curr_grapheme: 0,
            curr_grapheme_back: 0,
        }
    }
}

impl<'a> GraphemeIter<'a> {
    pub fn new(data: &'a str, offsets: &'a [usize]) -> Self {
        GraphemeIter {
            data,
            offsets,
            curr_grapheme: 0,
            curr_grapheme_back: offsets.len(),
        }
    }

    pub fn new_bytes(bytes: &'a [u8], offsets: &'a [usize]) -> Self {
        if let Ok(data) = from_utf8(bytes) {
            GraphemeIter {
                data,
                offsets,
                curr_grapheme: 0,
                curr_grapheme_back: offsets.len(),
            }
        } else {
            GraphemeIter {
                data: "",
                offsets,
                curr_grapheme: 0,
                curr_grapheme_back: 0,
            }
        }
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
