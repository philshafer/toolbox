//! Size-capped, time-limited LRU caches for rendered pages and
//! filtered images.
//!
//! Angel caches more eagerly than a normal proxy: a household reading
//! the news rarely needs a page fresher than an hour, and the bar's
//! "reload" link fetches a new copy on demand.  Entries live at least
//! `min` (default 60 minutes), longer when the origin asks for it, up
//! to `max`.
//!
//! Eviction scans for the least recently used entry.  That is linear,
//! but the caches hold at most a few thousand entries and the scan
//! only runs when an insert overflows the cap.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Cache<V> {
    inner: Mutex<Inner<V>>,
    cap: usize,
    min: Duration,
    max: Duration,
}

struct Inner<V> {
    map: HashMap<String, Entry<V>>,
    bytes: usize,
    tick: u64,
}

struct Entry<V> {
    val: Arc<V>,
    stored: Instant,
    expires: Instant,
    used: u64,
    size: usize,
}

/// A hit: the value and how long ago it was stored.
pub struct Hit<V> {
    pub val: Arc<V>,
    pub age: Duration,
}

impl<V> Cache<V> {
    /// `cap` is in bytes.  `max` is raised to `min` if it is smaller.
    pub fn new(cap: usize, min: Duration, max: Duration) -> Cache<V> {
        Cache {
            inner: Mutex::new(Inner { map: HashMap::new(), bytes: 0, tick: 0 }),
            cap,
            min,
            max: max.max(min),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<V>> {
        // A panic while holding the lock leaves the map consistent
        // (every update is a single insert/remove), so keep going.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, key: &str) -> Option<Hit<V>> {
        let now = Instant::now();
        let mut g = self.lock();
        g.tick += 1;
        let tick = g.tick;
        let e = g.map.get_mut(key)?;
        if e.expires <= now {
            let size = e.size;
            g.map.remove(key);
            g.bytes -= size;
            return None;
        }
        e.used = tick;
        Some(Hit { val: Arc::clone(&e.val), age: now - e.stored })
    }

    /// Lifetime for an entry the origin wants kept `origin` seconds.
    pub fn ttl(&self, origin: Option<u64>) -> Duration {
        let o = Duration::from_secs(origin.unwrap_or(0));
        o.clamp(self.min, self.max)
    }

    /// Store `val`, which takes about `size` bytes.  Values bigger than
    /// an eighth of the cache are not stored.
    pub fn put(&self, key: &str, val: V, size: usize, ttl: Duration) -> Arc<V> {
        let val = Arc::new(val);
        let size = size + key.len() + 64;
        if self.cap == 0 || size > self.cap / 8 {
            return val;
        }
        let now = Instant::now();
        let mut g = self.lock();
        g.tick += 1;
        let tick = g.tick;
        if let Some(old) = g.map.remove(key) {
            g.bytes -= old.size;
        }
        // Expired entries go first, then least recently used.
        if g.bytes + size > self.cap {
            g.map.retain(|_, e| e.expires > now);
            g.bytes = g.map.values().map(|e| e.size).sum();
        }
        while g.bytes + size > self.cap {
            let Some(k) = g.map.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else {
                break;
            };
            if let Some(e) = g.map.remove(&k) {
                g.bytes -= e.size;
            }
        }
        g.bytes += size;
        let e = Entry { val: Arc::clone(&val), stored: now, expires: now + ttl, used: tick, size };
        g.map.insert(key.to_string(), e);
        val
    }

    pub fn remove(&self, key: &str) {
        let mut g = self.lock();
        if let Some(e) = g.map.remove(key) {
            g.bytes -= e.size;
        }
    }
}

/// "just now", "12 min ago", "3 h ago".
pub fn age_text(age: Duration) -> String {
    let m = age.as_secs() / 60;
    match m {
        0 => "just now".into(),
        1..=119 => format!("{m} min ago"),
        _ => format!("{} h ago", m / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn hit_miss_replace() {
        let c: Cache<String> = Cache::new(10_000, HOUR, HOUR);
        assert!(c.get("a").is_none());
        c.put("a", "one".into(), 3, HOUR);
        assert_eq!(*c.get("a").unwrap().val, "one");
        c.put("a", "two".into(), 3, HOUR);
        assert_eq!(*c.get("a").unwrap().val, "two");
        c.remove("a");
        assert!(c.get("a").is_none());
    }

    #[test]
    fn expiry() {
        let c: Cache<u32> = Cache::new(10_000, Duration::ZERO, HOUR);
        c.put("a", 1, 10, Duration::ZERO);
        assert!(c.get("a").is_none());
    }

    #[test]
    fn evicts_lru() {
        // Each entry is 100 + key + 64 bytes; room for eight.
        let c: Cache<u32> = Cache::new(8 * 170, HOUR, HOUR);
        for k in ["a", "b", "c"] {
            c.put(k, 0, 100, HOUR);
        }
        c.get("a");
        for i in 0..10 {
            c.put(&format!("x{i}"), 0, 100, HOUR);
            c.get("a");
        }
        assert!(c.get("a").is_some());
        assert!(c.get("b").is_none());
        assert!(c.lock().bytes <= c.cap);
    }

    #[test]
    fn too_big_not_stored() {
        let c: Cache<u32> = Cache::new(800, HOUR, HOUR);
        c.put("a", 0, 500, HOUR);
        assert!(c.get("a").is_none());
    }

    #[test]
    fn ttl_bounds() {
        let c: Cache<u32> = Cache::new(800, HOUR, 24 * HOUR);
        assert_eq!(c.ttl(None), HOUR);
        assert_eq!(c.ttl(Some(60)), HOUR);
        assert_eq!(c.ttl(Some(7200)), 2 * HOUR);
        assert_eq!(c.ttl(Some(10_000_000)), 24 * HOUR);
    }
}
