pub mod compare;
pub mod query;
pub mod search;
pub mod types;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::types::{Crate, CrateMetadata};
use std::fmt::Display;

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Index {
    pub crates: HashMap<CrateMetadata, Crate>,
    pub parents: HashMap<CrateMetadata, HashMap<types::Id, Parent>>,
}
#[derive(Clone, Copy, Debug, Encode, Decode)]
pub enum Parent {
    Module(types::Id),
    Struct(types::Id),
    Trait(types::Id),
    Impl(types::Id),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Path {
    pub name: String,
    pub modules: Vec<types::Item>,
    pub owner: Option<types::Item>,
    pub item: types::Item,
}

impl Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for m in &self.modules {
            if let Some(name) = &m.name {
                write!(f, "{}::", name)?;
            }
        }
        if let Some(owner) = &self.owner {
            if let Some(name) = &owner.name {
                write!(f, "{}::", name)?;
            }
        }

        write!(f, "{}", self.item.name.as_deref().unwrap_or(""))?;

        Ok(())
    }
}

impl Path {
    pub fn pathify(&self) -> Vec<String> {
        let mut path = Vec::new();
        for m in &self.modules {
            if let Some(name) = &m.name {
                path.push(name.clone());
            }
        }
        if let Some(owner) = &self.owner {
            if let Some(name) = &owner.name {
                path.push(name.clone());
            }
        }
        if let Some(name) = &self.item.name {
            path.push(name.clone());
        }
        path
    }
    pub fn link(&self) -> String {
        let mut link = String::new();
        if self.name == "std" || self.name == "core" || self.name == "alloc" {
            link.push_str("https://doc.rust-lang.org/");
        } else {
            link.push_str(format!("https://docs.rs/{}/latest/", self.name).as_str());
        }
        for m in &self.modules {
            if let Some(name) = &m.name {
                link.push_str(&format!("{}/", name));
            }
        }
        if let Some(owner) = &self.owner {
            match &owner.inner {
                types::ItemEnum::Struct(_) => {
                    link.push_str("struct.");
                }
                types::ItemEnum::Trait(_) => {
                    link.push_str("trait.");
                }
                types::ItemEnum::Impl(_) => {
                    link.push_str("impl.");
                }
                _ => {}
            }
            link.push_str(&format!("{}.html#", owner.name.as_deref().unwrap_or("")));
            link.push_str(&format!(
                "method.{}.html",
                self.item.name.as_deref().unwrap_or("")
            ));
        } else {
            link.push_str(&format!(
                "fn.{}.html",
                self.item.name.as_deref().unwrap_or("")
            ));
        }
        link
    }
}

pub fn build_parent_index(krate: &types::Crate) -> HashMap<types::Id, Parent> {
    let mut parent = HashMap::new();
    for (id, item) in &krate.index {
        match &item.inner {
            types::ItemEnum::Primitive(p) => {
                for child in &p.impls {
                    parent.insert(*child, Parent::Module(*id));
                }
            }
            types::ItemEnum::Module(m) => {
                for child in &m.items {
                    parent.insert(*child, Parent::Module(*id));
                }
            }
            types::ItemEnum::Struct(s) => {
                for child in &s.impls {
                    parent.insert(*child, Parent::Struct(*id));
                }
            }
            types::ItemEnum::Trait(t) => {
                for child in &t.items {
                    parent.insert(*child, Parent::Trait(*id));
                }
            }
            types::ItemEnum::Impl(i) => {
                for child in &i.items {
                    parent.insert(*child, Parent::Impl(*id));
                }
            }
            _ => {}
        }
    }
    tracing::info!(
        "Built parent index for crate {}",
        krate.name.clone().unwrap()
    );
    // println!("{:#?}", parent);
    parent
}

/// Fallback: reconstruct a lexical module path for *local* items.
fn reconstruct_path_for_local(
    krate: &types::Crate,
    id: &types::Id,
    parents: &HashMap<types::Id, Parent>,
) -> Option<Path> {
    // Start from the item itself: push its own name if it has one (non-root modules/items).
    let mut cur = *id;
    let item = krate.index.get(&cur).unwrap().clone();

    let mut path = Path {
        name: krate.name.clone().unwrap_or_default(),
        modules: vec![],
        owner: None,
        item: item.clone(),
    };

    // Walk up through modules until crate root.
    let mut walker = Some(cur);
    while let Some(here) = walker {
        match parents.get(&here) {
            Some(Parent::Module(mid)) => {
                cur = *mid;
                let mi = &krate.index[mid];
                if let types::ItemEnum::Module(m) = &mi.inner {
                    if m.is_crate {
                        // reached the root module; prepend crate name and stop
                        path.modules.push(mi.clone());
                        break;
                    }
                }
                if let Some(_mname) = mi.name.as_deref() {
                    path.modules.push(mi.clone());
                }
                walker = Some(cur);
            }
            // If the immediate parent is a Trait/Impl, keep climbing—those don’t contribute
            // to the *path on disk* (HTML lives under the module tree).
            Some(Parent::Trait(tid)) | Some(Parent::Impl(tid)) | Some(Parent::Struct(tid)) => {
                walker = Some(*tid);
                path.owner = Some(krate.index.get(tid).unwrap().clone());
            }
            None => break,
        }
    }

    path.modules.reverse();
    Some(path)
}
