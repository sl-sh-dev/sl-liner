use super::*;

use std::{
    collections::{vec_deque, VecDeque},
    io::{BufRead, BufReader, BufWriter},
    fs::File,
    io::{self, Write},
    iter::IntoIterator,
    ops::Index,
    ops::IndexMut,
    path::Path,
    //time::Duration,
};

const DEFAULT_MAX_SIZE: usize = 1000;

/// Structure encapsulating command history
pub struct History {
    // TODO: this should eventually be private
    /// Vector of buffers to store history in
    pub buffers: VecDeque<Buffer>,
    /// Store a filename to save history into; if None don't save history
    file_name: Option<String>,
    /// Maximal number of buffers stored in the memory
    /// TODO: just make this public?
    max_buffers_size: usize,
    /// Maximal number of lines stored in the file
    // TODO: just make this public?
    max_file_size: usize,
    // TODO set from environment variable?
    pub append_duplicate_entries: bool,
    // Location for reverse search.
    reverse_location: usize,
}

impl History {
    /// Create new History structure.
    pub fn new() -> History {
        History {
            buffers: VecDeque::with_capacity(DEFAULT_MAX_SIZE),
            file_name: None,
            max_buffers_size: DEFAULT_MAX_SIZE,
            max_file_size: DEFAULT_MAX_SIZE,
            append_duplicate_entries: false,
            reverse_location: 0,
        }
    }

    /// Set history file name and at the same time load the history.
    pub fn set_file_name_and_load_history<P: AsRef<Path>>(&mut self, path: P) -> io::Result<String> {
        let status;
        let path = path.as_ref();
        let file = if path.exists() {
            status = format!("opening {:?}", path);
            File::open(path)?
        } else {
            status = format!("creating {:?}", path);
            File::create(path)?
        };
        let reader = BufReader::new(file);
        for line in reader.lines() {
            match line {
                Ok(line) => self.buffers.push_back(Buffer::from(line)),
                Err(_) => break,
            }
        }
        self.file_name = path.to_str().map(|s| s.to_owned());
        Ok(status)
    }

    /// Set maximal number of buffers stored in memory
    pub fn set_max_buffers_size(&mut self, size: usize) {
        self.max_buffers_size = size;
    }

    /// Set maximal number of entries in history file
    pub fn set_max_file_size(&mut self, size: usize) {
        self.max_file_size = size;
    }

    /// Number of items in history.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    /// Add a command to the history buffer and remove the oldest commands when the max history
    /// size has been met. If writing to the disk is enabled, this function will be used for
    /// logging history to the designated history file.
    pub fn push(&mut self, new_item: Buffer) -> io::Result<()> {
        // buffers[0] is the oldest entry
        // the new entry goes to the end
        if !self.append_duplicate_entries
            && self.buffers.back().map(|b| b.to_string()) == Some(new_item.to_string())
        {
            return Ok(());
        }

        self.buffers.push_back(new_item);
        while self.buffers.len() > self.max_buffers_size {
            self.buffers.pop_front();
        }
        Ok(())
    }

    /// Removes duplicate entries in the history
    pub fn remove_duplicates(&mut self, input: &str) {
        self.buffers.retain(|buffer| {
            let command = buffer.lines().concat();
            command != input
        });
    }

    /// Go through the history and try to find a buffer which starts the same as the new buffer
    /// given to this function as argument.
    pub fn get_newest_match<'a, 'b>(
        &'a self,
        curr_position: Option<usize>,
        new_buff: &'b Buffer,
    ) -> Option<&'a Buffer> {
        let pos = curr_position.unwrap_or_else(|| self.buffers.len());
        for iter in (0..pos).rev() {
            if let Some(tested) = self.buffers.get(iter) {
                if tested.starts_with(new_buff) {
                    return self.buffers.get(iter);
                }
            }
        }
        None
    }

    pub fn init_reverse_search(&mut self, search_term: String) {
        self.reverse_location = self.len();
        if let Some(idx) = self.reverse_search_index(search_term) {
            self.reverse_location = idx+1;
        }
    }

    /// Go through the history and try to find a buffer index that contains search_term.
    pub fn reverse_search_index(
        &self,
        search_term: String,
    ) -> Option<usize> {
        for iter in (0..self.reverse_location).rev() {
            if let Some(tested) = self.buffers.get(iter) {
                let stested: String = tested.to_string();
                if stested.contains(&search_term) {
                    return Some(iter);
                }
            }
        }
        if self.reverse_location != self.len() {
            // Wrap around if not found.
            for iter in (self.reverse_location..self.len()).rev() {
                if let Some(tested) = self.buffers.get(iter) {
                    let stested: String = tested.to_string();
                    if stested.contains(&search_term) {
                        return Some(iter);
                    }
                }
            }

        }
        None
    }

    /// Go through the history and try to find a buffer which starts the same as the new buffer
    /// given to this function as argument.
    pub fn reverse_search<'a, 'b>(
        &'a self,
        search_term: String,
    ) -> Option<&'a Buffer> {
        if let Some(idx) = self.reverse_search_index(search_term) {
            self.buffers.get(idx)
        } else {
            None
        }
    }

    pub fn next_reverse_search(&mut self, search_term: String) {
        if self.reverse_location > 0 {
            self.reverse_location -= 1;
        } else {
            // Wrap around if we hit bottom.
            self.reverse_location = self.len();
        }
        if let Some(idx) = self.reverse_search_index(search_term.clone()) {
            self.reverse_location = idx+1;
        /*} else {
            if self.reverse_location != self.len() {
                self.reverse_location = self.len();
                if let Some(idx) = self.reverse_search_index(search_term) {
                    self.reverse_location = idx+1;
                }
            }*/
        }
    }

    /// Get the history file name.
    #[inline(always)]
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_ref().map(|s| s.as_str())
    }

    pub fn commit_to_file(&mut self) {
        if let Some(file_name) = self.file_name.clone() {
            // Find how many bytes we need to move backwards
            // in the file to remove all the old commands.
            if self.buffers.len() >= self.max_file_size {
                let pop_out = self.buffers.len() - self.max_file_size;
                for _ in 0..pop_out {
                    self.buffers.pop_front();
                }
            }

            let mut file = BufWriter::new(File::create(&file_name)
                // It's safe to unwrap, because the file has be loaded by this time
                .unwrap());

            // Write the commands to the history file.
            for command in self.buffers.iter().cloned() {
                let _ = file.write_all(&String::from(command).as_bytes());
                let _ = file.write_all(b"\n");
            }
        }
    }
}

impl<'a> IntoIterator for &'a History {
    type Item = &'a Buffer;
    type IntoIter = vec_deque::Iter<'a, Buffer>;

    fn into_iter(self) -> Self::IntoIter {
        self.buffers.iter()
    }
}

impl Index<usize> for History {
    type Output = Buffer;

    fn index(&self, index: usize) -> &Buffer {
        &self.buffers[index]
    }
}

impl IndexMut<usize> for History {
    fn index_mut(&mut self, index: usize) -> &mut Buffer {
        &mut self.buffers[index]
    }
}
