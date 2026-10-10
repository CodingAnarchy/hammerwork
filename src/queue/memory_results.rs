//! In-memory storage for job results configured with
//! [`ResultStorage::Memory`](crate::job::ResultStorage::Memory).
//!
//! Each [`JobQueue`](super::JobQueue) owns one store, shared by all its clones and so by
//! every worker running on that queue. Results kept here live only in this process: other
//! processes (the CLI, the web dashboard, workers in other processes) cannot see them, and
//! they are lost when the process exits.
//!
//! The store is bounded. Entries expire at their TTL (checked on read, and pruned as new
//! results are stored), and when the store is full the entry that expires soonest is
//! evicted, the oldest first among entries without a TTL. Every operation is
//! `O(log n)` amortised: pruning only removes entries that have already expired, from
//! the front of an ordered index, so a write never scans the whole store.

// Only the database backends read results back.
#![cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]

use crate::job::JobId;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};

/// The number of results a [`JobQueue`](super::JobQueue) keeps in memory by default.
///
/// Change it with
/// [`JobQueue::with_memory_result_capacity`](super::JobQueue::with_memory_result_capacity).
pub const DEFAULT_MEMORY_RESULT_CAPACITY: usize = 10_000;

/// Eviction order: soonest expiry first (entries without a TTL sort last), then oldest
/// insertion first. The sequence number makes every key unique.
type EvictionKey = (DateTime<Utc>, u64);

#[derive(Debug)]
struct Entry {
    value: serde_json::Value,
    expires_at: Option<DateTime<Utc>>,
    key: EvictionKey,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<JobId, Entry>,
    order: BTreeMap<EvictionKey, JobId>,
    next_seq: u64,
}

impl Inner {
    fn remove(&mut self, job_id: &JobId) -> Option<Entry> {
        let entry = self.entries.remove(job_id)?;
        self.order.remove(&entry.key);
        Some(entry)
    }

    /// Removes the entries that have expired at `now`. Expired entries are at the front
    /// of `order`, so this only touches entries it removes (plus one).
    fn prune_expired(&mut self, now: DateTime<Utc>) -> u64 {
        let mut removed = 0;
        while let Some((&(expiry, _), _)) = self.order.first_key_value() {
            // Entries without a TTL sort at `MAX_UTC` and never expire.
            if expiry == DateTime::<Utc>::MAX_UTC || expiry > now {
                break;
            }
            if let Some((_, job_id)) = self.order.pop_first() {
                self.entries.remove(&job_id);
                removed += 1;
            }
        }
        removed
    }
}

/// A bounded, TTL-aware map from job id to result. See the [module docs](self).
#[derive(Debug)]
pub(crate) struct MemoryResultStore {
    capacity: usize,
    inner: Mutex<Inner>,
}

impl Default for MemoryResultStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_MEMORY_RESULT_CAPACITY)
    }
}

impl MemoryResultStore {
    /// A store that keeps at most `capacity` results (none when `capacity` is 0).
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Mutex::new(Inner::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // The critical sections cannot leave `Inner` inconsistent, so a panic in another
        // thread holding the lock is no reason to fail here.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores `value` as the result of `job_id`, replacing any earlier result. It expires
    /// at `expires_at` (never when `None`).
    pub(crate) fn store(
        &self,
        job_id: JobId,
        value: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) {
        self.store_at(Utc::now(), job_id, value, expires_at);
    }

    fn store_at(
        &self,
        now: DateTime<Utc>,
        job_id: JobId,
        value: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = self.lock();
        inner.remove(&job_id);
        if expires_at.is_some_and(|exp| exp <= now) {
            return;
        }
        inner.prune_expired(now);
        while inner.entries.len() >= self.capacity {
            match inner.order.pop_first() {
                Some((_, evicted)) => {
                    inner.entries.remove(&evicted);
                }
                None => break,
            }
        }
        let seq = inner.next_seq;
        inner.next_seq = inner.next_seq.wrapping_add(1);
        let key = (expires_at.unwrap_or(DateTime::<Utc>::MAX_UTC), seq);
        inner.order.insert(key, job_id);
        inner.entries.insert(
            job_id,
            Entry {
                value,
                expires_at,
                key,
            },
        );
    }

    /// The unexpired result of `job_id`, if any. An expired result is removed.
    pub(crate) fn get(&self, job_id: JobId) -> Option<serde_json::Value> {
        self.get_at(Utc::now(), job_id)
    }

    fn get_at(&self, now: DateTime<Utc>, job_id: JobId) -> Option<serde_json::Value> {
        let mut inner = self.lock();
        let entry = inner.entries.get(&job_id)?;
        if entry.expires_at.is_some_and(|exp| exp <= now) {
            inner.remove(&job_id);
            return None;
        }
        Some(entry.value.clone())
    }

    /// Removes the result of `job_id`. Returns whether there was one.
    pub(crate) fn remove(&self, job_id: JobId) -> bool {
        self.lock().remove(&job_id).is_some()
    }

    /// Removes every expired result and returns how many were removed.
    pub(crate) fn cleanup_expired(&self) -> u64 {
        self.lock().prune_expired(Utc::now())
    }

    #[cfg(test)]
    fn cleanup_expired_at(&self, now: DateTime<Utc>) -> u64 {
        self.lock().prune_expired(now)
    }

    /// The number of results held, including expired ones not yet pruned.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;
    use uuid::Uuid;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn stores_and_reads_a_result() {
        let store = MemoryResultStore::default();
        let id = Uuid::new_v4();
        store.store(id, json!({"ok": true}), None);
        assert_eq!(store.get(id), Some(json!({"ok": true})));
        assert_eq!(store.get(Uuid::new_v4()), None);
        assert_eq!(store.capacity(), DEFAULT_MEMORY_RESULT_CAPACITY);
    }

    #[test]
    fn expires_on_read_after_ttl() {
        let store = MemoryResultStore::with_capacity(10);
        let id = Uuid::new_v4();
        store.store_at(t0(), id, json!(1), Some(t0() + Duration::seconds(10)));
        assert_eq!(
            store.get_at(t0() + Duration::seconds(9), id),
            Some(json!(1))
        );
        assert_eq!(store.get_at(t0() + Duration::seconds(10), id), None);
        // The expired entry was removed by the read.
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn already_expired_result_is_not_stored() {
        let store = MemoryResultStore::with_capacity(10);
        let id = Uuid::new_v4();
        store.store_at(t0(), id, json!(1), Some(t0()));
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn writes_prune_expired_entries() {
        let store = MemoryResultStore::with_capacity(10);
        let short = Uuid::new_v4();
        let long = Uuid::new_v4();
        let forever = Uuid::new_v4();
        store.store_at(t0(), short, json!(1), Some(t0() + Duration::seconds(1)));
        store.store_at(t0(), long, json!(2), Some(t0() + Duration::hours(1)));
        store.store_at(t0(), forever, json!(3), None);
        assert_eq!(store.len(), 3);

        store.store_at(t0() + Duration::seconds(5), Uuid::new_v4(), json!(4), None);
        assert_eq!(store.len(), 3, "the expired entry is pruned by the write");
        assert_eq!(store.get_at(t0() + Duration::seconds(5), short), None);
        assert_eq!(
            store.get_at(t0() + Duration::seconds(5), long),
            Some(json!(2))
        );
        assert_eq!(
            store.get_at(t0() + Duration::days(365), forever),
            Some(json!(3))
        );
    }

    #[test]
    fn cleanup_removes_only_expired_entries() {
        let store = MemoryResultStore::with_capacity(10);
        for secs in [1, 2, 3] {
            store.store_at(
                t0(),
                Uuid::new_v4(),
                json!(secs),
                Some(t0() + Duration::seconds(secs)),
            );
        }
        store.store_at(t0(), Uuid::new_v4(), json!("forever"), None);
        assert_eq!(store.cleanup_expired_at(t0() + Duration::seconds(2)), 2);
        assert_eq!(store.len(), 2);
        assert_eq!(store.cleanup_expired_at(t0() + Duration::days(1)), 1);
        assert_eq!(store.len(), 1);
        assert_eq!(store.cleanup_expired(), 0);
    }

    #[test]
    fn full_store_evicts_soonest_to_expire_first() {
        let store = MemoryResultStore::with_capacity(3);
        let soon = Uuid::new_v4();
        let later = Uuid::new_v4();
        let never = Uuid::new_v4();
        store.store_at(t0(), never, json!("never"), None);
        store.store_at(t0(), later, json!("later"), Some(t0() + Duration::hours(2)));
        store.store_at(t0(), soon, json!("soon"), Some(t0() + Duration::hours(1)));

        let new = Uuid::new_v4();
        store.store_at(t0(), new, json!("new"), Some(t0() + Duration::hours(3)));
        assert_eq!(store.len(), 3);
        assert_eq!(
            store.get_at(t0(), soon),
            None,
            "soonest to expire is evicted"
        );
        assert!(store.get_at(t0(), later).is_some());
        assert!(store.get_at(t0(), never).is_some());
        assert!(store.get_at(t0(), new).is_some());
    }

    #[test]
    fn full_store_without_ttls_evicts_oldest_first() {
        let store = MemoryResultStore::with_capacity(2);
        let ids: Vec<_> = (0..4).map(|_| Uuid::new_v4()).collect();
        for (n, id) in ids.iter().enumerate() {
            store.store_at(t0(), *id, json!(n), None);
        }
        assert_eq!(store.len(), 2);
        assert_eq!(store.get_at(t0(), ids[0]), None);
        assert_eq!(store.get_at(t0(), ids[1]), None);
        assert_eq!(store.get_at(t0(), ids[2]), Some(json!(2)));
        assert_eq!(store.get_at(t0(), ids[3]), Some(json!(3)));
    }

    #[test]
    fn overwrite_replaces_value_and_expiry() {
        let store = MemoryResultStore::with_capacity(2);
        let id = Uuid::new_v4();
        store.store_at(t0(), id, json!("old"), Some(t0() + Duration::seconds(1)));
        store.store_at(t0(), id, json!("new"), None);
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.get_at(t0() + Duration::days(1), id),
            Some(json!("new"))
        );

        // The overwritten entry's old eviction key is gone: filling the store evicts
        // the (refreshed) entry only by its new position.
        let other = Uuid::new_v4();
        store.store_at(t0(), other, json!("other"), Some(t0() + Duration::hours(1)));
        store.store_at(t0(), Uuid::new_v4(), json!("third"), None);
        assert_eq!(store.get_at(t0(), other), None);
        assert_eq!(store.get_at(t0(), id), Some(json!("new")));
    }

    #[test]
    fn remove_deletes_a_result() {
        let store = MemoryResultStore::with_capacity(2);
        let id = Uuid::new_v4();
        store.store(id, json!(1), None);
        assert!(store.remove(id));
        assert!(!store.remove(id));
        assert_eq!(store.get(id), None);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn zero_capacity_keeps_nothing() {
        let store = MemoryResultStore::with_capacity(0);
        let id = Uuid::new_v4();
        store.store(id, json!(1), None);
        assert_eq!(store.get(id), None);
    }
}
