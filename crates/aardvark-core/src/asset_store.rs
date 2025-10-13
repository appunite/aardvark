use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// In-memory registry of assets exposed to the embedded JavaScript runtime.
#[derive(Clone, Default)]
pub struct AssetStore {
    inner: Rc<AssetStoreInner>,
}

#[derive(Default)]
struct AssetStoreInner {
    entries: RefCell<HashMap<String, Asset>>,
}

/// Asset payload stored in the registry.
#[derive(Clone)]
pub enum Asset {
    Text(Arc<str>),
    Binary(Arc<[u8]>),
}

impl AssetStore {
    /// Creates an empty asset store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts (or replaces) a text asset.
    pub fn insert_text(&self, name: &str, contents: impl Into<Arc<str>>) {
        let mut entries = self.inner.entries.borrow_mut();
        entries.insert(name.to_owned(), Asset::Text(contents.into()));
    }

    /// Inserts (or replaces) a binary asset.
    pub fn insert_bytes(&self, name: &str, bytes: impl Into<Arc<[u8]>>) {
        let mut entries = self.inner.entries.borrow_mut();
        entries.insert(name.to_owned(), Asset::Binary(bytes.into()));
    }

    /// Returns the asset matching the provided name, if present.
    pub fn get(&self, name: &str) -> Option<Asset> {
        let entries = self.inner.entries.borrow();
        if let Some(asset) = entries.get(name) {
            return Some(asset.clone());
        }
        if let Some(stripped) = name.strip_prefix("./") {
            if let Some(asset) = entries.get(stripped) {
                return Some(asset.clone());
            }
        }
        if let Some(last) = name.rsplit('/').next() {
            if let Some(asset) = entries.get(last) {
                return Some(asset.clone());
            }
        }
        None
    }
}
