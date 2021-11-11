pub struct GraphemeIter<'a> {
    data: &'a str,
    offsets: &'a [usize],
    curr_grapheme: usize,
}

impl<'a> GraphemeIter<'a> {
    pub fn new(data: &'a str, offsets: &'a [usize]) -> Self {
        GraphemeIter {
            data,
            offsets,
            curr_grapheme: 0,
        }
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
}
