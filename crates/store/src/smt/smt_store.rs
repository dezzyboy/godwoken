//! Implement SMTStore trait

use std::sync::Arc;

use crate::traits::KVStore;
use dashmap::DashMap;
use gw_common::{
    sparse_merkle_tree::{
        error::Error as SMTError,
        traits::Store,
        tree::{BranchKey, BranchNode},
    },
    H256,
};
use gw_db::{error::Error, schema::Col, RocksDBWriteBatch};
use gw_types::{packed, prelude::*, store};

pub struct SMTStore<'a, DB: KVStore> {
    leaf_col: Col,
    branch_col: Col,
    store: &'a DB,
}

impl<'a, DB: KVStore> SMTStore<'a, DB> {
    pub fn new(leaf_col: Col, branch_col: Col, store: &'a DB) -> Self {
        SMTStore {
            leaf_col,
            branch_col,
            store,
        }
    }

    pub fn inner_store(&self) -> &DB {
        self.store
    }
}

impl<'a, DB: KVStore> Store<H256> for SMTStore<'a, DB> {
    fn get_branch(&self, branch_key: &BranchKey) -> Result<Option<BranchNode>, SMTError> {
        let branch_key: packed::SMTBranchKey = branch_key.pack();
        match self.store.get(self.branch_col, branch_key.as_slice()) {
            Some(slice) => {
                let smt_branch = store::SMTBranchNode::uncheck_from_slice(slice.as_ref());
                Ok(Some(BranchNode::from(&smt_branch)))
            }
            None => Ok(None),
        }
    }

    fn get_leaf(&self, leaf_key: &H256) -> Result<Option<H256>, SMTError> {
        match self.store.get(self.leaf_col, leaf_key.as_slice()) {
            Some(slice) if 32 == slice.len() => {
                let mut leaf = [0u8; 32];
                leaf.copy_from_slice(slice.as_ref());
                Ok(Some(H256::from(leaf)))
            }
            Some(_) => Err(SMTError::Store("get corrupted leaf".to_string())),
            None => Ok(None),
        }
    }

    fn insert_branch(&mut self, branch_key: BranchKey, branch: BranchNode) -> Result<(), SMTError> {
        let branch_key: packed::SMTBranchKey = branch_key.pack();
        let branch = store::SMTBranchNode::from(&branch);

        self.store
            .insert_raw(self.branch_col, branch_key.as_slice(), branch.as_slice())
            .map_err(|err| SMTError::Store(format!("insert error {}", err)))?;

        Ok(())
    }

    fn insert_leaf(&mut self, leaf_key: H256, leaf: H256) -> Result<(), SMTError> {
        self.store
            .insert_raw(self.leaf_col, leaf_key.as_slice(), leaf.as_slice())
            .map_err(|err| SMTError::Store(format!("insert error {}", err)))?;

        Ok(())
    }

    fn remove_branch(&mut self, branch_key: &BranchKey) -> Result<(), SMTError> {
        let branch_key: packed::SMTBranchKey = branch_key.pack();

        self.store
            .delete(self.branch_col, branch_key.as_slice())
            .map_err(|err| SMTError::Store(format!("delete error {}", err)))?;

        Ok(())
    }

    fn remove_leaf(&mut self, leaf_key: &H256) -> Result<(), SMTError> {
        self.store
            .delete(self.leaf_col, leaf_key.as_slice())
            .map_err(|err| SMTError::Store(format!("delete error {}", err)))?;

        Ok(())
    }
}

pub enum CacheValue<V> {
    Exists(V),
    Deleted,
    None,
}

#[derive(Clone)]
pub struct SMTCache {
    pub(crate) branches: Arc<DashMap<BranchKey, CacheValue<BranchNode>>>,
    pub(crate) leaves: Arc<DashMap<H256, CacheValue<H256>>>,
}

impl SMTCache {
    pub fn write(
        &self,
        leaf_col: Col,
        branch_col: Col,
        deleted_flag: u8,
        write_batch: &mut RocksDBWriteBatch,
    ) -> Result<(), Error> {
        let mut branch_count = 0;
        for branch in self.branches.iter() {
            let key: packed::SMTBranchKey = branch.key().pack();
            match branch.value() {
                CacheValue::Exists(node) => {
                    let node = store::SMTBranchNode::from(node);
                    write_batch.put(branch_col, key.as_slice(), node.as_slice())?;
                    branch_count += 1;
                }
                CacheValue::Deleted => {
                    write_batch.put(branch_col, key.as_slice(), &[deleted_flag])?;
                    branch_count += 1;
                }
                CacheValue::None => {}
            }
        }

        let mut leaf_count = 0;
        for leaf in self.leaves.iter() {
            let key = leaf.key();
            match leaf.value() {
                CacheValue::Exists(leaf) => {
                    write_batch.put(leaf_col, key.as_slice(), leaf.as_slice())?;
                    leaf_count += 1;
                }
                CacheValue::Deleted => {
                    write_batch.put(leaf_col, key.as_slice(), &[deleted_flag])?;
                    leaf_count += 1;
                }
                CacheValue::None => {}
            }
        }

        log::info!(
            "smt cache write branches {}, leaves {}, total {}",
            branch_count,
            leaf_count,
            write_batch.len(),
        );

        Ok(())
    }

    pub fn clear(&self) {
        self.branches.clear();
        self.leaves.clear();
    }
}

impl Default for SMTCache {
    fn default() -> Self {
        SMTCache {
            branches: Arc::new(DashMap::new()),
            leaves: Arc::new(DashMap::new()),
        }
    }
}
