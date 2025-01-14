use leveldb::{
    database::Database,
    kv::KV,
    options::{Options, ReadOptions, WriteOptions},
};
use lru::LruCache;
use rmp_serde::Serializer;
use serde::Serialize as _;
use std::num::NonZeroUsize;

use crate::nodes::{MetaData, Node};

const META_DATA_KEY: i32 = !0;

#[derive(Debug)]
pub enum Error {
    Serialize(rmp_serde::encode::Error),
    Deserialize(rmp_serde::decode::Error),
    LevelDB(leveldb::error::Error),
    //UnknownError,
}

impl From<rmp_serde::encode::Error> for Error {
    fn from(e: rmp_serde::encode::Error) -> Self {
        Self::Serialize(e)
    }
}

impl From<rmp_serde::decode::Error> for Error {
    fn from(e: rmp_serde::decode::Error) -> Self {
        Self::Deserialize(e)
    }
}

impl From<leveldb::error::Error> for Error {
    fn from(e: leveldb::error::Error) -> Self {
        Self::LevelDB(e)
    }
}
pub trait Storage<T, const M: usize, const M0: usize>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + std::fmt::Debug,
{
    fn num_layer(&mut self) -> usize;
    fn num_node(&self) -> usize;
    fn update_entry_point(&mut self, id: i32) -> Result<(), Error>;
    fn load_entry_point_node(&mut self) -> Option<Node<T, M, M0>>;
    fn store_new_node(&mut self, node: Node<T, M, M0>) -> Result<(), Error>;
    fn meta_data(&self) -> &MetaData;
    fn get(&mut self, id: i32) -> Result<Option<Node<T, M, M0>>, Error>;
    fn put(&mut self, node: Node<T, M, M0>) -> Result<(), Error>;
}

pub struct HashMap<T, const M: usize, const M0: usize>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + std::fmt::Debug,
{
    map: std::collections::HashMap<i32, Node<T, M, M0>>,
    meta_data: MetaData,
}

impl<T, const M: usize, const M0: usize> HashMap<T, M, M0>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + std::fmt::Debug,
{
    pub fn new() -> Self {
        Self {
            map: Default::default(),
            meta_data: MetaData {
                entry_point: None,
                num_nodes: None,
            },
        }
    }
}

impl<T, const M: usize, const M0: usize> Default for HashMap<T, M, M0>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + std::fmt::Debug,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const M: usize, const M0: usize> Storage<T, M, M0> for HashMap<T, M, M0>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + std::fmt::Debug,
{
    fn num_layer(&mut self) -> usize {
        self.load_entry_point_node()
            .map_or(0, |node| node.neighbors.len())
    }

    fn num_node(&self) -> usize {
        self.meta_data.num_nodes.unwrap_or(0) as usize
    }

    fn update_entry_point(&mut self, id: i32) -> Result<(), Error> {
        self.meta_data.entry_point = Some(id);

        Ok(())
    }

    fn load_entry_point_node(&mut self) -> Option<Node<T, M, M0>> {
        self.meta_data
            .entry_point
            .and_then(|ep| self.map.get(&ep).cloned())
    }

    fn store_new_node(&mut self, node: Node<T, M, M0>) -> Result<(), Error> {
        self.meta_data.num_nodes = Some(self.meta_data.num_nodes.map_or(1, |num| num + 1));
        self.meta_data.entry_point.get_or_insert(node.id as i32);

        self.put(node)
    }

    fn meta_data(&self) -> &MetaData {
        &self.meta_data
    }

    fn get(&mut self, id: i32) -> Result<Option<Node<T, M, M0>>, Error> {
        Ok(self.map.get(&id).cloned())
    }

    fn put(&mut self, node: Node<T, M, M0>) -> Result<(), Error> {
        self.map.insert(node.id as i32, node);

        Ok(())
    }
}

pub struct NodeDB<T, const M: usize, const M0: usize>
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::clone::Clone + core::fmt::Debug,
{
    meta_data: MetaData,
    db: Database<i32>,
    cache: LruCache<i32, Node<T, M, M0>>,
}

impl<T, const M: usize, const M0: usize> Storage<T, M, M0> for NodeDB<T, M, M0>
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::clone::Clone + core::fmt::Debug,
{
    fn num_layer(&mut self) -> usize {
        if let Some(ep) = self.meta_data.entry_point {
            match self.get(ep) {
                Ok(Some(node)) => node.neighbors.len(),
                _ => 0,
            }
        } else {
            0
        }
    }

    fn num_node(&self) -> usize {
        if let Some(n) = self.meta_data.num_nodes {
            n as usize
        } else {
            0
        }
    }

    fn update_entry_point(&mut self, id: i32) -> Result<(), Error> {
        self.meta_data.entry_point = Some(id);
        self.save_metadata()
    }

    fn load_entry_point_node(&mut self) -> Option<crate::nodes::Node<T, M, M0>> {
        if let Some(ep) = self.meta_data.entry_point {
            match self.get(ep) {
                Ok(Some(node)) => Some(node),
                _ => None,
            }
        } else {
            None
        }
    }

    fn store_new_node(&mut self, node: crate::nodes::Node<T, M, M0>) -> Result<(), Error> {
        self.meta_data.num_nodes = match self.meta_data.num_nodes {
            Some(n) => Some(n + 1),
            None => Some(1),
        };
        self.meta_data.entry_point = match self.meta_data.entry_point {
            Some(ep) => Some(ep),
            None => Some(node.id as i32),
        };

        self.put(node)
    }

    fn meta_data(&self) -> &MetaData {
        &self.meta_data
    }

    fn get(&mut self, id: i32) -> Result<Option<crate::nodes::Node<T, M, M0>>, Error> {
        if let Some(val) = self.cache.get_mut(&id) {
            return Ok(Some(val.clone()));
        }

        if let Some(data) = self.db.get(ReadOptions::new(), id)? {
            let node: Node<T, M, M0> = rmp_serde::from_slice(&data[..])?;
            self.cache.put(id, node.clone());

            Ok(Some(node))
        } else {
            Ok(None)
        }
    }

    fn put(&mut self, node: crate::nodes::Node<T, M, M0>) -> Result<(), Error> {
        let mut buf = Vec::new();
        node.serialize(&mut Serializer::new(&mut buf))?;

        self.db.put(WriteOptions::new(), node.id as i32, &buf)?;
        self.cache.put(node.id as i32, node);

        self.save_metadata()
    }
}

impl<T, const M: usize, const M0: usize> NodeDB<T, M, M0>
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::clone::Clone + core::fmt::Debug,
{
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        let db = Database::open(path.as_ref(), {
            let mut opts = Options::new();
            opts.create_if_missing = true;

            opts
        })
        .unwrap();

        let cache = LruCache::new(NonZeroUsize::new(10000).unwrap());
        let meta_data = match db.get(ReadOptions::new(), META_DATA_KEY) {
            Ok(Some(data)) => rmp_serde::from_slice(&data[..]).unwrap(),
            _ => MetaData {
                entry_point: None,
                num_nodes: None,
            },
        };

        NodeDB {
            meta_data,
            db,
            cache,
        }
    }

    fn save_metadata(&self) -> Result<(), Error> {
        let mut buf = Vec::new();
        self.meta_data.serialize(&mut Serializer::new(&mut buf))?;

        let _ = self.db.put(WriteOptions::new(), META_DATA_KEY, &buf);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn load_meta_data(&mut self) -> Result<Option<crate::nodes::Node<T, M, M0>>, Error> {
        match self.meta_data.entry_point {
            Some(ep) => self.get(ep),
            _ => Ok(None),
        }
    }
}
