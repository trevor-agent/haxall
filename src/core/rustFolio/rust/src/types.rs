// types.rs — Haystack value type representations.
//
// Dict is stored as a sorted Vec<(String, Val)> for:
//   - Deterministic serialization order (required for wire protocol)
//   - Cache-friendly layout for iteration (filter eval iterates all tags)
//   - Binary search gives O(log n) lookup — folio records have 5-50 tags

use chrono::{NaiveDate, NaiveTime};
use chrono::DateTime;
use chrono_tz::Tz;

/// All storable Haystack value types.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Marker,
    NA,
    Remove,                              // None.val — tag removal sentinel
    Bool(bool),
    Number(f64, Option<String>),         // value + optional unit
    Str(String),
    Ref(HRef),
    Uri(String),
    Date(NaiveDate),
    Time(NaiveTime),
    DateTime(DateTime<Tz>),
    Coord(f64, f64),                     // lat, lng
    XStr(String, String),               // type, val
    Symbol(String),
    Span(String),                        // Span.toStr representation
    List(Vec<Val>),
    Dict(Dict),
    Grid(Grid),
}

/// Haystack Ref — id string + optional display string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HRef {
    pub id: String,
    pub dis: Option<String>,
}

impl HRef {
    pub fn new(id: impl Into<String>) -> Self {
        HRef { id: id.into(), dis: None }
    }

    pub fn with_dis(id: impl Into<String>, dis: impl Into<String>) -> Self {
        HRef { id: id.into(), dis: Some(dis.into()) }
    }
}

impl std::fmt::Display for HRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.id)
    }
}

/// Ordered tag map — sorted alphabetically by name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict {
    pub tags: Vec<(String, Val)>,
}

impl Dict {
    pub fn new() -> Self {
        Dict { tags: Vec::new() }
    }

    pub fn from_tags(mut tags: Vec<(String, Val)>) -> Self {
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        Dict { tags }
    }

    /// Get value by name (binary search — O(log n)).
    pub fn get(&self, name: &str) -> Option<&Val> {
        self.tags
            .binary_search_by(|(k, _)| k.as_str().cmp(name))
            .ok()
            .map(|i| &self.tags[i].1)
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Get "id" tag as HRef.
    pub fn id(&self) -> Option<&HRef> {
        match self.get("id") {
            Some(Val::Ref(r)) => Some(r),
            _ => None,
        }
    }

    /// Get id string.
    pub fn id_str(&self) -> Option<&str> {
        self.id().map(|r| r.id.as_str())
    }

    /// Get "mod" tag as DateTime.
    pub fn mod_time(&self) -> Option<&DateTime<Tz>> {
        match self.get("mod") {
            Some(Val::DateTime(dt)) => Some(dt),
            _ => None,
        }
    }

    /// Set or replace a tag (maintains sorted order).
    pub fn set(&mut self, name: impl Into<String>, val: Val) {
        let name = name.into();
        match self.tags.binary_search_by(|(k, _)| k.as_str().cmp(&name)) {
            Ok(i)  => self.tags[i].1 = val,
            Err(i) => self.tags.insert(i, (name, val)),
        }
    }

    /// Remove a tag by name.
    pub fn remove(&mut self, name: &str) {
        if let Ok(i) = self.tags.binary_search_by(|(k, _)| k.as_str().cmp(name)) {
            self.tags.remove(i);
        }
    }

    /// Merge other's tags onto self — other's values take precedence.
    /// Tags with Val::Remove in other are removed from the result.
    pub fn merge(&self, other: &Dict) -> Dict {
        let mut result = self.clone();
        for (name, val) in &other.tags {
            if matches!(val, Val::Remove) {
                result.remove(name);
            } else {
                result.set(name.clone(), val.clone());
            }
        }
        result
    }

    /// Apply a changes dict as a diff:
    /// - Val::Remove entries delete the tag
    /// - All other entries set the tag
    pub fn apply_changes(&self, changes: &Dict) -> Dict {
        self.merge(changes)
    }

    pub fn len(&self) -> usize {
        self.tags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Val)> {
        self.tags.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn has_marker(&self, name: &str) -> bool {
        matches!(self.get(name), Some(Val::Marker))
    }

    pub fn is_trash(&self) -> bool {
        self.has_marker("trash")
    }
}

/// Grid — meta + column names + rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub meta: Dict,
    pub cols: Vec<String>,
    pub rows: Vec<Dict>,
}

/// Diff flags (mirror Fantom Diff.fan constants).
pub mod diff_flags {
    pub const ADD:               u8 = 0x01;
    pub const REMOVE:            u8 = 0x02;
    pub const TRANSIENT:         u8 = 0x04;
    pub const FORCE:             u8 = 0x08;
    pub const BYPASS_RESTRICTED: u8 = 0x10;
    pub const CUR_VAL:           u8 = 0x20;
    pub const POINT:             u8 = 0x40;
    pub const TREE_UPDATE:       u8 = 0x80;
}

/// A single diff — change to apply to a record.
#[derive(Debug, Clone)]
pub struct Diff {
    pub id:      HRef,
    pub old_mod: Option<DateTime<Tz>>,  // None for add
    pub changes: Dict,
    pub flags:   u8,
}

impl Diff {
    pub fn is_add(&self)       -> bool { self.flags & diff_flags::ADD      != 0 }
    pub fn is_remove(&self)    -> bool { self.flags & diff_flags::REMOVE   != 0 }
    pub fn is_transient(&self) -> bool { self.flags & diff_flags::TRANSIENT != 0 }
    pub fn is_force(&self)     -> bool { self.flags & diff_flags::FORCE    != 0 }
    pub fn is_update(&self)    -> bool { !self.is_add() && !self.is_remove() }
}

/// Result of applying a single diff.
#[derive(Debug, Clone)]
pub struct CommitResult {
    pub id:      HRef,
    pub old_mod: Option<DateTime<Tz>>,
    pub new_mod: Option<DateTime<Tz>>,  // None for transient commits
    pub old_rec: Option<Dict>,           // None for adds
    pub new_rec: Option<Dict>,           // None for removes
}
