use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{self, Write},
    io::{BufRead, BufReader, BufWriter},
    ops::Index,
    path::Path,
};

const DEFAULT_MAX_SIZE: usize = 1000;

#[derive(Clone, Debug)]
struct HistoryItem {
    context: Option<Vec<String>>,
    buffer: String,
}

impl HistoryItem {
    fn merge_context(&mut self, other_context: &Option<Vec<String>>, max_contexts: usize) {
        if let Some(my_context) = &mut self.context {
            let mut has_wild = false;
            if let Some(context) = other_context {
                let astrik = "*".to_string();
                if my_context.contains(&astrik) || context.contains(&astrik) {
                    has_wild = true;
                } else {
                    for ctx in context {
                        if !my_context.contains(ctx) {
                            my_context.push(ctx.to_string());
                        }
                    }
                }
            }
            if my_context.len() > max_contexts || has_wild {
                my_context.clear();
                my_context.push("*".to_string());
            }
        } else {
            self.context = other_context.clone();
        }
    }
}

/// Structure encapsulating command history
pub struct History {
    /// Vector of buffers to store history in
    buffers: VecDeque<HistoryItem>,
    /// Store a filename to save history into; if None don't save history
    file_name: Option<String>,
    /// Maximum number of history items
    max_history_size: usize,
    /// Last filesize of history file, used to optimize history sharing.
    file_size: u64,
    /// Writes between history compaction.
    compaction_writes: usize,
    /// How many "throwaway" history items to remove on a push.
    throwaways: usize,
    /// When sharing history keep this many local items at top of history (session pushes).
    local_share: usize,
    /// Max number of contexts for an item before it is just * (wildcard).
    max_contexts: usize,
    /// The current context to use for history searches.
    search_context: Option<String>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// Create new History structure.
    pub fn new() -> History {
        History {
            buffers: VecDeque::with_capacity(DEFAULT_MAX_SIZE),
            file_name: None,
            max_history_size: DEFAULT_MAX_SIZE,
            file_size: 0,
            compaction_writes: 0,
            throwaways: 0,
            local_share: 0,
            max_contexts: 5,
            search_context: None,
        }
    }

    pub fn set_max_contexts(&mut self, n: usize) {
        self.max_contexts = n;
    }

    pub fn set_search_context(&mut self, context: Option<String>) {
        self.search_context = context;
    }

    /// Clears out the history.
    pub fn clear_history(&mut self) {
        self.buffers.clear();
    }

    /// Loads the history file from the saved path and appends it to the end of the history if append
    /// is true otherwise replace history.
    pub fn load_history(&mut self, append: bool) -> io::Result<()> {
        if let Some(path) = self.file_name.clone() {
            let file_size = self.file_size;
            self.load_history_file_test(&path, file_size, append)
                .map(|l| {
                    self.file_size = l;
                })
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "History filename not set!",
            ))
        }
    }

    /// Loads the history file from path and appends it to the end of the history if append is true.
    pub fn load_history_file<P: AsRef<Path>>(&mut self, path: P, append: bool) -> io::Result<u64> {
        self.load_history_file_test(path, 0, append)
    }

    /// Loads the history file from path and appends it to the end of the history if append is true
    /// (replaces if false).  Only loads if length is not equal to current file size.
    fn load_history_file_test<P: AsRef<Path>>(
        &mut self,
        path: P,
        length: u64,
        append: bool,
    ) -> io::Result<u64> {
        let path = path.as_ref();
        let file = if path.exists() {
            File::open(path)?
        } else {
            let status = format!("File not found {:?}", path);
            return Err(io::Error::new(io::ErrorKind::Other, status));
        };
        let new_length = file.metadata()?.len();
        if new_length == 0 && length == 0 && !append {
            // Special case, trying to load nothing and not appending- just clear.
            self.clear_history();
        }
        if new_length != length {
            let mut dups: HashMap<String, (usize, Option<Vec<String>>)> = HashMap::new();
            let local_buffers: Option<Vec<HistoryItem>> = if !append && self.local_share > 0 {
                let mut local_buffers = Vec::with_capacity(self.local_share);
                let mut i = 0;
                while let Some(buf) = self.buffers.pop_back() {
                    local_buffers.push(buf);
                    i += 1;
                    if i == self.local_share {
                        break;
                    }
                }
                Some(local_buffers)
            } else {
                None
            };
            if !append {
                self.clear_history();
            }
            self.load_buffers(&path, &mut dups)?;
            // Put any locally added history on "top" and take care of context.
            if let Some(mut local_buffers) = local_buffers {
                while let Some(buf) = local_buffers.pop() {
                    self.add_buffer(buf, &mut dups);
                }
            }
            self.buffers.retain(|buf| {
                if let Some(mut dup) = dups.get_mut(&buf.buffer) {
                    if dup.0 > 1 {
                        dup.0 -= 1;
                        false
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
            self.truncate();
        }
        Ok(new_length)
    }

    fn add_buffer(
        &mut self,
        mut buf: HistoryItem,
        dups: &mut HashMap<String, (usize, Option<Vec<String>>)>,
    ) {
        let mut dup = dups.entry(buf.buffer.clone()).or_insert((0, None));
        dup.0 += 1;
        buf.merge_context(&dup.1, self.max_contexts);
        dup.1 = buf.context.clone();
        self.buffers.push_back(buf);
    }

    fn load_buffers<P: AsRef<Path>>(
        &mut self,
        path: &P,
        dups: &mut HashMap<String, (usize, Option<Vec<String>>)>,
    ) -> io::Result<()> {
        let path = path.as_ref();
        let file = if path.exists() {
            File::open(path)?
        } else {
            let status = format!("File not found {:?}", path);
            return Err(io::Error::new(io::ErrorKind::Other, status));
        };
        let reader = BufReader::new(file);
        let mut context: Option<Vec<String>> = None;
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if line.starts_with("#<ctx>") && line.len() > 6 {
                        let mut cvec = Vec::new();
                        for c in line[6..].split(':') {
                            cvec.push(c.to_string());
                        }
                        if cvec.len() > self.max_contexts {
                            cvec.clear();
                            cvec.push("*".to_string());
                        }
                        context = if !cvec.is_empty() { Some(cvec) } else { None }
                    } else if !line.starts_with('#') {
                        let buf = HistoryItem {
                            context,
                            buffer: line,
                        };
                        self.add_buffer(buf, dups);
                        context = None;
                    }
                }
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Set history file name and at the same time load the history.
    pub fn set_file_name_and_load_history<P: AsRef<Path>>(&mut self, path: P) -> io::Result<u64> {
        let path = path.as_ref();
        self.file_name = path.to_str().map(|s| s.to_owned());
        self.file_size = 0;
        if path.exists() {
            self.load_history_file(path, false).map(|l| {
                self.file_size = l;
                l
            })
        } else {
            File::create(path)?;
            Ok(0)
        }
    }

    /// Set maximal number of buffers in history
    pub fn set_max_history_size(&mut self, size: usize) {
        self.max_history_size = size;
    }

    /// Number of items in history.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    /// Is the history empty
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Adds a "throwaway" history item.  Any of these will be removed once push
    /// is called.  Intended to allow "error" or other bad items to stick around
    /// long enough for the user to correct without cluttering history long term.
    pub fn push_throwaway(&mut self, new_item: impl Into<String>) -> io::Result<()> {
        let new_item: String = new_item.into();
        // buffers[0] is the oldest entry
        // the new entry goes to the end
        if self.buffers.back().map(|b| b.buffer.to_string()) == Some(new_item.to_string()) {
            return Ok(());
        }

        self.buffers.push_back(HistoryItem {
            context: None,
            buffer: new_item,
        });
        self.throwaways += 1;
        Ok(())
    }

    /// Add a command to the history buffer and remove the oldest commands when the max history
    /// size has been met. If writing to the disk is enabled, this function will be used for
    /// logging history to the designated history file.
    pub fn push(&mut self, new_item: impl Into<String>) -> io::Result<()> {
        let new_item: String = new_item.into();
        // buffers[0] is the oldest entry
        // the new entry goes to the end

        // Remove any throwaway items first.
        while self.throwaways > 0 {
            self.buffers.pop_back();
            self.throwaways -= 1;
        }
        self.local_share += 1;
        let mut same_last_context = true;
        if let Some(context) = &self.search_context {
            if let Some(Some(last_context)) = self.buffers.back().map(|b| &b.context) {
                same_last_context = last_context.contains(context);
            }
        }
        if self.buffers.back().map(|b| b.buffer.to_string()) == Some(new_item.to_string())
            && same_last_context
        {
            return Ok(());
        }

        let item_str = new_item.to_string(); //String::from(new_item.clone());
        let context = if let Some(mut old_context) = self.remove_duplicates(&item_str) {
            let mut has_wild = false;
            if let Some(context) = &self.search_context {
                let astrik = "*".to_string();
                if old_context.contains(&astrik) || context.contains(&astrik) {
                    has_wild = true;
                } else if !old_context.contains(context) {
                    old_context.push(context.to_string());
                }
            }
            if old_context.len() > self.max_contexts || has_wild {
                old_context.clear();
                old_context.push("*".to_string());
            }
            Some(old_context)
        } else if let Some(context) = &self.search_context {
            let c = vec![context.to_string()];
            Some(c)
        } else {
            None
        };

        if let Some(file_name) = &self.file_name {
            if let Ok(inner_file) = std::fs::OpenOptions::new().append(true).open(&file_name) {
                // Leave file size alone, if it is not right trigger a reload later.
                let mut file = BufWriter::new(inner_file);
                // Save the filesize after each append so we do not reload when we do not need to.
                self.file_size += History::write_item(&mut file, &context, &item_str) as u64;
            }
        }
        self.buffers.push_back(HistoryItem {
            context,
            buffer: new_item,
        });
        while self.buffers.len() > self.max_history_size {
            self.buffers.pop_front();
        }
        if self.file_name.is_some() {
            let _ = self.load_history(false);
            // Do not want duplicates so periodically compact the history file.
            self.compaction_writes += 1;
            // Every 30 writes "compact" the history file by writing just in memory history.  This
            // is to keep the history file clean and at a reasonable size (not much over max
            // history size at it's worst).
            if self.compaction_writes > 29 {
                // Not using shared history so just de-dup the file without messing with
                // our history.
                if self.file_name.is_some() {
                    let _ = self.commit_to_file();
                }
                self.compaction_writes = 0;
            }
        }
        Ok(())
    }

    /// Removes duplicate entries in the history
    fn remove_duplicates(&mut self, input: &str) -> Option<Vec<String>> {
        let mut ret = None;
        self.buffers.retain(|buffer| {
            let command = &buffer.buffer; // XXX.lines().concat();
            if command == input {
                ret = buffer.context.clone();
            }
            command != input
        });
        ret
    }

    fn get_match<I>(&self, vals: I, search_term: &str) -> Option<usize>
    where
        I: Iterator<Item = usize>,
    {
        let mut candidate = None;
        for v in vals {
            if let Some(tested) = self.buffers.get(v) {
                if tested.buffer.starts_with(search_term) {
                    if candidate.is_none() {
                        candidate = Some(v);
                    }
                    if let Some(search_context) = &self.search_context {
                        if let Some(context) = &tested.context {
                            if context.contains(&"*".to_string())
                                || context.contains(search_context)
                            {
                                return Some(v);
                            }
                        }
                    } else if candidate.is_some() {
                        return candidate;
                    }
                }
            }
        }
        candidate
    }

    /// Go through the history and try to find an index (newest to oldest) which starts the same
    /// as the new buffer given to this function as argument.  Starts at curr_position.  Does not wrap.
    pub fn get_newest_match(&self, curr_position: Option<usize>, new_buff: &str) -> Option<usize> {
        let pos = curr_position.unwrap_or_else(|| self.buffers.len());
        if pos > 0 {
            self.get_match((0..pos).rev(), new_buff)
        } else {
            None
        }
    }

    pub fn get_history_subset(&self, search_term: &str) -> Vec<usize> {
        let mut v1: Vec<usize> = Vec::new();
        let mut v2: Vec<usize> = Vec::new();
        let mut ret: Vec<usize> = (0..self.len())
            .filter(|i| {
                if let Some(tested) = self.buffers.get(*i) {
                    let starts = tested.buffer.starts_with(search_term);
                    let contains = tested.buffer.contains(search_term);
                    let has_context = if let Some(context) = &self.search_context {
                        if let Some(con_list) = &tested.context {
                            con_list.contains(&"*".to_string()) || con_list.contains(context)
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if has_context && starts {
                        v1.push(*i);
                    } else if starts {
                        v2.push(*i);
                    }
                    contains && !starts && tested.buffer != search_term
                } else {
                    false
                }
            })
            .collect();
        ret.append(&mut v2);
        ret.append(&mut v1);
        ret
    }

    pub fn search_index(&self, search_term: &str) -> Vec<usize> {
        (0..self.len())
            .filter_map(|i| self.buffers.get(i).map(|t| (i, t)))
            .filter(|(_i, tested)| tested.buffer.contains(search_term))
            .map(|(i, _)| i)
            .collect()
    }

    /// Get the history file name.
    #[inline(always)]
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    fn truncate(&mut self) {
        // Find how many lines we need to move backwards
        // in the file to remove all the old commands.
        if self.buffers.len() >= self.max_history_size {
            let pop_out = self.buffers.len() - self.max_history_size;
            for _ in 0..pop_out {
                self.buffers.pop_front();
            }
        }
    }

    fn write_item(file: &mut dyn Write, context: &Option<Vec<String>>, item: &str) -> usize {
        let mut ret = 0;
        if let Some(context) = context {
            let _ = file.write_all(b"#<ctx>");
            ret += 6;
            let mut first = true;
            for ctx in context {
                if !first {
                    let _ = file.write_all(b":");
                    ret += 1;
                }
                let _ = file.write_all(ctx.as_bytes());
                ret += ctx.as_bytes().len();
                first = false;
            }
            let _ = file.write_all(b"\n");
            ret += 1;
        }
        let _ = file.write_all(item.as_bytes());
        let _ = file.write_all(b"\n");
        ret += item.as_bytes().len() + 1;
        ret
    }

    fn write_buffers<P: AsRef<Path>>(buffers: &VecDeque<HistoryItem>, path: P) -> io::Result<()> {
        let mut file = BufWriter::new(File::create(&path)?);

        // Write the commands to the history file.
        for command in buffers.iter().cloned() {
            let _ = History::write_item(&mut file, &command.context, &command.buffer);
        }
        Ok(())
    }

    pub fn commit_to_file_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        self.truncate();
        History::write_buffers(&self.buffers, path)
    }

    pub fn commit_to_file(&mut self) -> io::Result<()> {
        if let Some(file_name) = self.file_name.clone() {
            self.commit_to_file_path(file_name)
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "History filename not set, can not commit!",
            ))
        }
    }

    pub fn get_context(&self, index: usize) -> &Option<Vec<String>> {
        &self.buffers[index].context
    }
}

impl Index<usize> for History {
    type Output = str;

    fn index(&self, index: usize) -> &str {
        &self.buffers[index].buffer // XXX
    }
}
