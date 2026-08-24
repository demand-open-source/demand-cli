use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};

use bitcoin::Txid;

/// Manually applied fee deltas, keyed by transaction ID.
pub static PRIORITIZED_TRANSACTIONS: TransactionStore<i64> = TransactionStore::new();

/// Fee deltas reported by the mempool.space accelerator, keyed by transaction ID.
pub static MEMPOOL_DOT_SPACE_ACCELERATED: TransactionStore<i64> = TransactionStore::new();

/// Bad transactions, keyed by transaction ID.
pub static BAD_TRANSACTIONS: TransactionStore<i64> = TransactionStore::new();

/// A process-wide, lazily initialized collection keyed by transaction ID.
pub struct TransactionStore<T> {
    entries: OnceLock<Mutex<HashMap<Txid, T>>>,
}

impl<T> TransactionStore<T> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: OnceLock::new(),
        }
    }

    fn entries(&self) -> &Mutex<HashMap<Txid, T>> {
        self.entries.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Records a value, replacing any value already stored for `txid`.
    pub fn record(&self, txid: Txid, value: T) {
        self.entries()
            .lock()
            .expect("transaction store mutex poisoned")
            .insert(txid, value);
    }

    /// Returns a clone of the value stored for `txid`, if present.
    pub fn get(&self, txid: &Txid) -> Option<T>
    where
        T: Clone,
    {
        self.entries()
            .lock()
            .expect("transaction store mutex poisoned")
            .get(txid)
            .cloned()
    }

    /// Replaces the value for `txid` only when it is already tracked.
    pub fn update_existing(&self, txid: &Txid, value: T) -> bool {
        let mut entries = self
            .entries()
            .lock()
            .expect("transaction store mutex poisoned");
        let Some(existing) = entries.get_mut(txid) else {
            return false;
        };

        *existing = value;
        true
    }

    /// Clones all currently stored entries into a point-in-time snapshot.
    pub fn snapshot(&self) -> HashMap<Txid, T>
    where
        T: Clone,
    {
        self.entries()
            .lock()
            .expect("transaction store mutex poisoned")
            .clone()
    }

    /// Returns a point-in-time snapshot of all stored transaction IDs.
    pub fn snapshot_txids(&self) -> HashSet<Txid> {
        self.entries()
            .lock()
            .expect("transaction store mutex poisoned")
            .keys()
            .copied()
            .collect()
    }

    /// Removes all stored transactions.
    pub fn clear(&self) {
        self.entries()
            .lock()
            .expect("transaction store mutex poisoned")
            .clear();
    }

    /// Removes the value stored for `txid`, if present.
    pub fn remove(&self, txid: &Txid) {
        self.entries()
            .lock()
            .expect("transaction store mutex poisoned")
            .remove(txid);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        fmt::Debug,
    };

    use bitcoin::Txid;

    use super::TransactionStore;

    fn assert_shared_operations<T>(store: &TransactionStore<T>, txid: Txid, value: T)
    where
        T: Clone + Debug + Eq,
    {
        store.record(txid, value.clone());
        assert_eq!(store.get(&txid), Some(value.clone()));
        assert_eq!(store.snapshot(), HashMap::from([(txid, value.clone())]));
        assert_eq!(store.snapshot_txids(), HashSet::from([txid]));

        assert!(store.update_existing(&txid, value.clone()));
        store.clear();
        assert!(store.snapshot().is_empty());

        store.record(txid, value.clone());
        store.remove(&txid);
        assert_eq!(store.get(&txid), None);
        assert!(!store.update_existing(&txid, value));
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn fee_delta_store_supports_shared_operations() {
        let txid = "0000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .expect("valid test txid");

        assert_shared_operations(&TransactionStore::new(), txid, 6_438_i64);
    }
}
