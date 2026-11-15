//! Pure queue model (spec §4.1 / §5.2): no I/O, fully unit-tested.
//!
//! `entries` are always kept in *playback order*. When shuffle is enabled the
//! entries are physically shuffled (current track moved to the front) and
//! every entry keeps its `seq` — a sort key describing the original order —
//! so disabling shuffle restores the original order exactly.

use mochy_core::models::Song;
use mochy_core::settings::RepeatMode;

#[derive(Debug, Clone)]
pub struct Entry {
    pub song: Song,
    /// Unique id within this app run (stable across reorders; egui ids, drag).
    pub uid: u64,
    /// Original-order sort key (used to un-shuffle).
    seq: f64,
}

#[derive(Debug, Clone)]
pub struct Queue {
    entries: Vec<Entry>,
    cur: Option<usize>,
    shuffle: bool,
    pub repeat: RepeatMode,
    next_uid: u64,
    rng: u64,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x1234_5678);
        Self::with_seed(seed)
    }

    /// Deterministic shuffles (tests).
    pub fn with_seed(seed: u64) -> Self {
        Self {
            entries: Vec::new(),
            cur: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            next_uid: 1,
            rng: seed | 1,
        }
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn make_entry(&mut self, song: Song, seq: f64) -> Entry {
        let uid = self.next_uid;
        self.next_uid += 1;
        Entry { song, uid, seq }
    }

    // ---- accessors --------------------------------------------------------

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn songs(&self) -> impl Iterator<Item = &Song> {
        self.entries.iter().map(|e| &e.song)
    }
    pub fn current_index(&self) -> Option<usize> {
        self.cur
    }
    pub fn current(&self) -> Option<&Entry> {
        self.cur.and_then(|i| self.entries.get(i))
    }
    pub fn current_song(&self) -> Option<&Song> {
        self.current().map(|e| &e.song)
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn is_last(&self) -> bool {
        matches!(self.cur, Some(i) if i + 1 == self.entries.len())
    }
    pub fn contains_id(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.song.id == id)
    }
    /// Mutate every queued copy of a song (e.g. like state, lyrics offset).
    pub fn for_each_song_mut(&mut self, id: &str, mut f: impl FnMut(&mut Song)) {
        for e in self.entries.iter_mut().filter(|e| e.song.id == id) {
            f(&mut e.song);
        }
    }

    // ---- replacing / adding ----------------------------------------------

    /// Replace the queue with `songs`, current = `start`. If `shuffle` is on
    /// the rest of the queue is shuffled around the start track.
    pub fn set(&mut self, songs: Vec<Song>, start: usize) {
        self.entries.clear();
        for (i, s) in songs.into_iter().enumerate() {
            let e = self.make_entry(s, i as f64);
            self.entries.push(e);
        }
        self.cur = if self.entries.is_empty() { None } else { Some(start.min(self.entries.len() - 1)) };
        if self.shuffle {
            self.shuffle_around_current();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.cur = None;
    }

    fn max_seq(&self) -> f64 {
        self.entries.iter().map(|e| e.seq).fold(-1.0, f64::max)
    }

    /// Append at the end of the queue (end of original order too).
    pub fn append(&mut self, songs: Vec<Song>) {
        let mut seq = self.max_seq();
        for s in songs {
            seq += 1.0;
            let e = self.make_entry(s, seq);
            self.entries.push(e);
        }
        if self.cur.is_none() && !self.entries.is_empty() {
            self.cur = Some(0);
        }
    }

    /// Insert right after the current track ("Play next").
    pub fn insert_next(&mut self, song: Song) {
        let Some(cur) = self.cur else {
            self.append(vec![song]);
            return;
        };
        let cur_seq = self.entries[cur].seq;
        // Original-order neighbour: smallest seq greater than the current one.
        let next_seq = self
            .entries
            .iter()
            .map(|e| e.seq)
            .filter(|&s| s > cur_seq)
            .fold(f64::INFINITY, f64::min);
        let seq = if next_seq.is_finite() { (cur_seq + next_seq) / 2.0 } else { cur_seq + 1.0 };
        let e = self.make_entry(song, seq);
        self.entries.insert(cur + 1, e);
    }

    // ---- navigation -------------------------------------------------------

    /// Index that `advance` would move to, without moving.
    /// `auto` = natural end of track (honours RepeatMode::One).
    pub fn peek_next(&self, auto: bool) -> Option<usize> {
        let cur = self.cur?;
        if auto && self.repeat == RepeatMode::One {
            return Some(cur);
        }
        if cur + 1 < self.entries.len() {
            Some(cur + 1)
        } else if self.repeat == RepeatMode::All && !self.entries.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    /// Move to the next track. Returns the new current index, `None` at the
    /// end of the queue (current is left unchanged).
    pub fn advance(&mut self, auto: bool) -> Option<usize> {
        let n = self.peek_next(auto)?;
        self.cur = Some(n);
        Some(n)
    }

    /// Move to the previous track (wraps with RepeatMode::All, otherwise
    /// stays on the first track).
    pub fn go_prev(&mut self) -> Option<usize> {
        let cur = self.cur?;
        let p = if cur > 0 {
            cur - 1
        } else if self.repeat == RepeatMode::All {
            self.entries.len() - 1
        } else {
            0
        };
        self.cur = Some(p);
        Some(p)
    }

    pub fn jump_to(&mut self, idx: usize) -> bool {
        if idx < self.entries.len() {
            self.cur = Some(idx);
            true
        } else {
            false
        }
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.repeat
    }

    // ---- shuffle ----------------------------------------------------------

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.shuffle_around_current();
        } else {
            let cur_uid = self.current().map(|e| e.uid);
            self.entries.sort_by(|a, b| a.seq.total_cmp(&b.seq));
            self.cur = cur_uid.and_then(|u| self.entries.iter().position(|e| e.uid == u));
        }
    }

    /// Mark the queue as shuffled while keeping the present order (used when
    /// restoring a persisted, already-shuffled queue).
    pub fn set_shuffle_flag_only(&mut self, on: bool) {
        self.shuffle = on;
    }

    /// Current track first, everything else Fisher–Yates shuffled after it.
    fn shuffle_around_current(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let cur = self.cur.unwrap_or(0);
        let current = self.entries.remove(cur);
        let n = self.entries.len();
        for i in (1..n).rev() {
            let j = (self.rand() % (i as u64 + 1)) as usize;
            self.entries.swap(i, j);
        }
        self.entries.insert(0, current);
        self.cur = Some(0);
    }

    // ---- editing ----------------------------------------------------------

    /// Remove the entry at `idx`. Returns `true` when the *current* track was
    /// removed (the caller should then load the new current, if any).
    pub fn remove(&mut self, idx: usize) -> bool {
        if idx >= self.entries.len() {
            return false;
        }
        self.entries.remove(idx);
        let Some(cur) = self.cur else { return false };
        if self.entries.is_empty() {
            self.cur = None;
            return true;
        }
        if idx < cur {
            self.cur = Some(cur - 1);
            false
        } else if idx == cur {
            // The track that followed takes its place (wrap to 0 at the end).
            self.cur = Some(if cur < self.entries.len() { cur } else { 0 });
            true
        } else {
            false
        }
    }

    /// Move entry `from` to position `to` (both in playback order).
    pub fn move_entry(&mut self, from: usize, to: usize) {
        let n = self.entries.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let cur_uid = self.current().map(|e| e.uid);
        let e = self.entries.remove(from);
        self.entries.insert(to, e);
        self.cur = cur_uid.and_then(|u| self.entries.iter().position(|e| e.uid == u));
        if !self.shuffle {
            // Playback order *is* the original order: renumber.
            for (i, e) in self.entries.iter_mut().enumerate() {
                e.seq = i as f64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(n: usize) -> Vec<Song> {
        (0..n).map(|i| Song::remote(format!("v{i}"), format!("Song {i}"))).collect()
    }

    fn ids(q: &Queue) -> Vec<String> {
        q.songs().map(|s| s.id.clone()).collect()
    }

    fn cur_id(q: &Queue) -> String {
        q.current_song().unwrap().id.clone()
    }

    #[test]
    fn next_and_end_with_repeat_off() {
        let mut q = Queue::with_seed(7);
        q.repeat = RepeatMode::Off;
        q.set(songs(3), 0);
        assert_eq!(q.advance(true), Some(1));
        assert_eq!(q.advance(false), Some(2));
        assert!(q.is_last());
        assert_eq!(q.advance(true), None);
        assert_eq!(q.current_index(), Some(2), "current stays at end");
    }

    #[test]
    fn repeat_all_wraps_both_ways() {
        let mut q = Queue::with_seed(7);
        q.repeat = RepeatMode::All;
        q.set(songs(3), 2);
        assert_eq!(q.advance(true), Some(0));
        assert_eq!(q.go_prev(), Some(2));
    }

    #[test]
    fn repeat_one_only_on_auto_advance() {
        let mut q = Queue::with_seed(7);
        q.repeat = RepeatMode::One;
        q.set(songs(3), 1);
        assert_eq!(q.advance(true), Some(1));
        assert_eq!(q.advance(false), Some(2));
    }

    #[test]
    fn prev_stays_on_first_without_repeat() {
        let mut q = Queue::with_seed(7);
        q.repeat = RepeatMode::Off;
        q.set(songs(3), 0);
        assert_eq!(q.go_prev(), Some(0));
        q.jump_to(2);
        assert_eq!(q.go_prev(), Some(1));
    }

    #[test]
    fn cycle_repeat_order() {
        let mut q = Queue::with_seed(1);
        q.repeat = RepeatMode::Off;
        assert_eq!(q.cycle_repeat(), RepeatMode::All);
        assert_eq!(q.cycle_repeat(), RepeatMode::One);
        assert_eq!(q.cycle_repeat(), RepeatMode::Off);
    }

    #[test]
    fn shuffle_keeps_current_first_and_restores_order() {
        let mut q = Queue::with_seed(42);
        q.set(songs(20), 5);
        q.set_shuffle(true);
        assert_eq!(q.current_index(), Some(0));
        assert_eq!(cur_id(&q), "v5");
        let mut shuffled = ids(&q);
        assert_ne!(shuffled, ids(&{
            let mut u = q.clone();
            u.set_shuffle(false);
            u
        }));
        shuffled.sort();
        let mut orig: Vec<String> = songs(20).into_iter().map(|s| s.id).collect();
        orig.sort();
        assert_eq!(shuffled, orig, "same multiset");
        // advance a bit, then unshuffle: original order, current preserved
        q.advance(false);
        let playing = cur_id(&q);
        q.set_shuffle(false);
        assert_eq!(ids(&q), songs(20).into_iter().map(|s| s.id).collect::<Vec<_>>());
        assert_eq!(cur_id(&q), playing);
    }

    #[test]
    fn set_while_shuffled_shuffles_around_start() {
        let mut q = Queue::with_seed(3);
        q.set_shuffle(true);
        q.set(songs(10), 4);
        assert_eq!(q.current_index(), Some(0));
        assert_eq!(cur_id(&q), "v4");
        assert_eq!(q.len(), 10);
    }

    #[test]
    fn insert_next_lands_after_current_and_survives_unshuffle() {
        let mut q = Queue::with_seed(9);
        q.set(songs(5), 1);
        q.insert_next(Song::remote("x", "X"));
        assert_eq!(ids(&q), vec!["v0", "v1", "x", "v2", "v3", "v4"]);

        let mut q = Queue::with_seed(9);
        q.set(songs(5), 1);
        q.set_shuffle(true);
        q.insert_next(Song::remote("x", "X"));
        assert_eq!(q.entries()[1].song.id, "x");
        q.set_shuffle(false);
        assert_eq!(ids(&q), vec!["v0", "v1", "x", "v2", "v3", "v4"]);
    }

    #[test]
    fn append_sets_current_on_empty_queue() {
        let mut q = Queue::with_seed(1);
        q.append(songs(2));
        assert_eq!(q.current_index(), Some(0));
        q.append(vec![Song::remote("z", "Z")]);
        assert_eq!(ids(&q), vec!["v0", "v1", "z"]);
    }

    #[test]
    fn remove_before_at_and_after_current() {
        let mut q = Queue::with_seed(1);
        q.set(songs(5), 2);
        assert!(!q.remove(0)); // before
        assert_eq!(cur_id(&q), "v2");
        assert_eq!(q.current_index(), Some(1));
        assert!(!q.remove(3)); // after (v4)
        assert_eq!(cur_id(&q), "v2");
        assert!(q.remove(1)); // current -> successor takes over
        assert_eq!(cur_id(&q), "v3");
        assert!(q.remove(1)); // remove last current -> wraps to 0
        assert_eq!(cur_id(&q), "v1");
        assert!(q.remove(0));
        assert!(q.is_empty());
        assert_eq!(q.current_index(), None);
        assert!(!q.remove(0));
    }

    #[test]
    fn move_entry_tracks_current() {
        let mut q = Queue::with_seed(1);
        q.set(songs(5), 1);
        q.move_entry(1, 4);
        assert_eq!(ids(&q), vec!["v0", "v2", "v3", "v4", "v1"]);
        assert_eq!(cur_id(&q), "v1");
        assert_eq!(q.current_index(), Some(4));
        q.move_entry(0, 2);
        assert_eq!(ids(&q), vec!["v2", "v3", "v0", "v4", "v1"]);
        assert_eq!(q.current_index(), Some(4));
        // out of range is a no-op
        q.move_entry(9, 0);
        assert_eq!(q.len(), 5);
        // reorder in non-shuffle mode becomes the new original order
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(ids(&q), vec!["v2", "v3", "v0", "v4", "v1"]);
    }

    #[test]
    fn uids_are_unique() {
        let mut q = Queue::with_seed(1);
        q.set(songs(3), 0);
        q.append(songs(3));
        let mut u: Vec<u64> = q.entries().iter().map(|e| e.uid).collect();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), 6);
    }
}
