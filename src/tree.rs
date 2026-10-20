//! A single resource list, with heterogeneous account/folder/message rows.
//! Branch identities are account-scoped; traversal is iterative and cycle-safe.
use crate::domain::*;
use std::collections::{HashMap, HashSet};

pub type FolderKey = (String, String);

#[derive(Clone, Debug, Default)]
pub struct FolderPage {
    pub loaded: bool,
    pub loading: bool,
    pub reserved: usize,
    pub next: Option<Cursor>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Account(usize),
    Folder(usize),
    Message(usize),
    More(usize),
    Status(usize),
}
#[derive(Clone, Debug)]
pub struct TreeRow {
    pub node: Node,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKey {
    Account(String),
    Folder(FolderKey),
    Message(MessageId),
    More(FolderKey),
    Status(FolderKey),
}

pub struct TreeData<'a> {
    pub accounts: &'a [Account],
    pub folders: &'a [Folder],
    pub messages: &'a [Message],
    pub visible_messages: &'a [usize],
    pub account: Option<&'a str>,
    pub expanded: &'a HashSet<FolderKey>,
    pub collapsed_accounts: &'a HashSet<String>,
    pub pages: &'a HashMap<FolderKey, FolderPage>,
    pub filter: &'a str,
    pub searching: bool,
    pub account_sort: Option<bool>,
}

pub fn folder_key(folder: &Folder) -> FolderKey {
    (folder.account.clone(), folder.id.clone())
}

impl TreeData<'_> {
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut mail: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for i in self.visible_messages {
            let m = &self.messages[*i];
            mail.entry((&m.id.account, &m.id.folder))
                .or_default()
                .push(*i);
        }
        let needle = self.filter.to_lowercase();
        let restricted = self.searching || !needle.is_empty();
        let by_key: HashMap<_, _> = self
            .folders
            .iter()
            .enumerate()
            .map(|(i, f)| ((f.account.as_str(), f.id.as_str()), i))
            .collect();
        let mut included = HashSet::new();
        if restricted {
            for f in self.folders {
                let matching = mail.contains_key(&(f.account.as_str(), f.id.as_str()))
                    || !self.searching
                        && format!("{} {} {}", f.account, f.id, f.name)
                            .to_lowercase()
                            .contains(&needle);
                if !matching {
                    continue;
                }
                let mut current = Some(f);
                while let Some(folder) = current {
                    if !included.insert((folder.account.as_str(), folder.id.as_str())) {
                        break;
                    }
                    current = folder
                        .parent
                        .as_deref()
                        .and_then(|p| by_key.get(&(folder.account.as_str(), p)))
                        .map(|i| &self.folders[*i]);
                }
            }
        }
        let mut accounts: Vec<_> = self
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| self.account.is_none_or(|id| id == a.id))
            .collect();
        if let Some(desc) = self.account_sort {
            accounts.sort_by(|(_, a), (_, b)| {
                if desc {
                    b.id.cmp(&a.id)
                } else {
                    a.id.cmp(&b.id)
                }
            });
        }
        let mut out = vec![];
        for (ai, account) in accounts {
            let indices: Vec<_> = self
                .folders
                .iter()
                .enumerate()
                .filter(|(_, f)| f.account == account.id)
                .map(|(i, _)| i)
                .collect();
            if restricted
                && !indices.iter().any(|i| {
                    included.contains(&(account.id.as_str(), self.folders[*i].id.as_str()))
                })
            {
                continue;
            }
            let account_depth = usize::from(self.account.is_none());
            if self.account.is_none() {
                out.push(TreeRow {
                    node: Node::Account(ai),
                    depth: 0,
                });
                if self.collapsed_accounts.contains(&account.id) {
                    continue;
                }
            }
            let ids: HashSet<_> = indices
                .iter()
                .map(|i| self.folders[*i].id.as_str())
                .collect();
            let roots: Vec<_> = indices
                .iter()
                .copied()
                .filter(|i| {
                    self.folders[*i]
                        .parent
                        .as_deref()
                        .is_none_or(|p| !ids.contains(p))
                })
                .collect();
            let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
            for i in &indices {
                if let Some(parent) = self.folders[*i].parent.as_deref() {
                    children.entry(parent).or_default().push(*i);
                }
            }
            let mut visited = HashSet::new();
            for seed in roots.into_iter().chain(indices.iter().copied()) {
                let mut stack = vec![(seed, account_depth, true)];
                while let Some((i, depth, parent_visible)) = stack.pop() {
                    if !visited.insert(i) {
                        continue;
                    }
                    let folder = &self.folders[i];
                    let key = folder_key(folder);
                    let visible = parent_visible
                        && (!restricted
                            || included.contains(&(account.id.as_str(), folder.id.as_str())));
                    let expanded = self.expanded.contains(&key);
                    if visible {
                        out.push(TreeRow {
                            node: Node::Folder(i),
                            depth,
                        });
                        if expanded {
                            if let Some(messages) =
                                mail.get(&(account.id.as_str(), folder.id.as_str()))
                            {
                                out.extend(messages.iter().map(|i| TreeRow {
                                    node: Node::Message(*i),
                                    depth: depth + 1,
                                }));
                            }
                            if !self.searching {
                                match self.pages.get(&key) {
                                    Some(page) if page.loading || page.error.is_some() => {
                                        out.push(TreeRow {
                                            node: Node::Status(i),
                                            depth: depth + 1,
                                        })
                                    }
                                    Some(page) if page.next.is_some() => out.push(TreeRow {
                                        node: Node::More(i),
                                        depth: depth + 1,
                                    }),
                                    Some(page) if page.loaded => {}
                                    _ if folder.total > 0 => out.push(TreeRow {
                                        node: Node::More(i),
                                        depth: depth + 1,
                                    }),
                                    _ => {}
                                }
                            }
                        }
                    }
                    if let Some(children) = children.get(folder.id.as_str()) {
                        for child in children.iter().rev() {
                            stack.push((*child, depth + 1, visible && expanded));
                        }
                    }
                }
            }
        }
        out
    }
}
