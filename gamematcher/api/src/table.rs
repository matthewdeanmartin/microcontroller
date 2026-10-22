//! A list of records that remembers which rows changed since they were last
//! saved, and how to undo the changes since a savepoint
//! (`spec/sprint-10-storage.md`).
//!
//! Reading is a plain slice (`Deref`). Every way to change a row goes through
//! this type: indexing mutably, [`Table::get_mut`], push, remove, retain.
//! There is no `DerefMut`, so the compiler finds every write. A row is cloned
//! for undo the first time it's touched after a save, so a change costs the
//! rows it touches, not the whole table.
use std::collections::BTreeSet;
use std::ops::{Deref, Index, IndexMut};

/// A row with a stable, never-reused id (its record key).
pub trait Row: Clone {
    fn id(&self) -> u32;
}

macro_rules! row {
    ($($t:ty),*) => {$(
        impl Row for $t {
            fn id(&self) -> u32 {
                self.id
            }
        }
    )*};
}
row!(
    crate::messages::StoredAccount,
    crate::messages::StoredGame,
    crate::messages::StoredPlan,
    crate::messages::StoredProvider
);

#[derive(Clone, Debug)]
enum Undo<T> {
    /// The row as it was before its first change since the last save.
    Changed(T),
    Added(u32),
    Removed(usize, T),
}

#[derive(Clone, Debug)]
pub struct Table<T> {
    rows: Vec<T>,
    undo: Vec<Undo<T>>,
    /// Rows with an undo entry since the last save (cloned once only).
    touched: BTreeSet<u32>,
    /// Rows to write (or remove) at the next save.
    dirty: BTreeSet<u32>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            undo: Vec::new(),
            touched: BTreeSet::new(),
            dirty: BTreeSet::new(),
        }
    }
}

impl<T: PartialEq> PartialEq for Table<T> {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows
    }
}

impl<T: Row> Table<T> {
    /// Rows as loaded from storage: nothing dirty.
    pub fn from_rows(rows: Vec<T>) -> Self {
        Self {
            rows,
            ..Self::default()
        }
    }

    fn touch(&mut self, i: usize) {
        let id = self.rows[i].id();
        self.dirty.insert(id);
        if self.touched.insert(id) {
            self.undo.push(Undo::Changed(self.rows[i].clone()));
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        let i = self.rows.iter().position(|r| r.id() == id)?;
        Some(&mut self[i])
    }

    pub fn push(&mut self, row: T) {
        let id = row.id();
        self.dirty.insert(id);
        self.touched.insert(id);
        self.undo.push(Undo::Added(id));
        self.rows.push(row);
    }

    pub fn remove(&mut self, i: usize) -> T {
        let row = self.rows.remove(i);
        self.dirty.insert(row.id());
        self.undo.push(Undo::Removed(i, row.clone()));
        row
    }

    pub fn pop(&mut self) -> Option<T> {
        let last = self.rows.len().checked_sub(1)?;
        Some(self.remove(last))
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        let mut i = 0;
        while i < self.rows.len() {
            if keep(&self.rows[i]) {
                i += 1;
            } else {
                self.remove(i);
            }
        }
    }

    /// Every row, each marked changed. For bulk edits only; prefer indexing
    /// the rows that really change.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        for i in 0..self.rows.len() {
            self.touch(i);
        }
        self.rows.iter_mut()
    }

    /// A point [`Table::rollback`] can return to. Rows touched before it
    /// are cloned again on their next change, so they roll back too.
    pub fn savepoint(&mut self) -> usize {
        self.touched.clear();
        self.undo.len()
    }

    /// Undoes every change made since `savepoint`.
    pub fn rollback(&mut self, savepoint: usize) {
        // Rows touched since are cloned again next time (always safe).
        self.touched.clear();
        while self.undo.len() > savepoint {
            match self.undo.pop().expect("checked above") {
                Undo::Changed(row) => {
                    let id = row.id();
                    if let Some(r) = self.rows.iter_mut().find(|r| r.id() == id) {
                        *r = row;
                    }
                }
                Undo::Added(id) => {
                    if let Some(i) = self.rows.iter().rposition(|r| r.id() == id) {
                        self.rows.remove(i);
                    }
                }
                Undo::Removed(i, row) => {
                    let i = i.min(self.rows.len());
                    self.rows.insert(i, row);
                }
            }
        }
    }

    /// Ids changed since the last save (present rows are written, absent
    /// ones removed).
    pub fn dirty(&self) -> &BTreeSet<u32> {
        &self.dirty
    }

    pub fn get(&self, id: u32) -> Option<&T> {
        self.rows.iter().find(|r| r.id() == id)
    }

    /// Everything is on flash: forget dirt and undo history.
    pub fn saved(&mut self) {
        self.dirty.clear();
        self.undo.clear();
        self.touched.clear();
    }

    /// Marks every row dirty (migration writes them all).
    pub fn mark_all(&mut self) {
        self.dirty.extend(self.rows.iter().map(Row::id));
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.rows.iter().map(Row::id)
    }
}

impl<T> Deref for Table<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.rows
    }
}

impl<T> Index<usize> for Table<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.rows[i]
    }
}

impl<T: Row> IndexMut<usize> for Table<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        self.touch(i);
        &mut self.rows[i]
    }
}

impl<'a, T> IntoIterator for &'a Table<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct R {
        id: u32,
        v: u32,
    }
    impl Row for R {
        fn id(&self) -> u32 {
            self.id
        }
    }

    fn table() -> Table<R> {
        Table::from_rows((1..=4).map(|id| R { id, v: 0 }).collect())
    }

    #[test]
    fn only_touched_rows_are_dirty() {
        let mut t = table();
        assert!(t.dirty().is_empty());
        let _ = t[1].v; // reading is free
        assert!(t.dirty().is_empty());
        t[2].v = 7;
        t.get_mut(4).unwrap().v = 1;
        assert_eq!(t.dirty().iter().copied().collect::<Vec<_>>(), [3, 4]);
        t.saved();
        assert!(t.dirty().is_empty());
    }

    #[test]
    fn rollback_restores_rows_and_order() {
        let mut t = table();
        let before = t.rows.clone();
        let sp = t.savepoint();
        t[0].v = 9;
        t[0].v = 10;
        t.push(R { id: 5, v: 5 });
        t.remove(1);
        t[0].v = 11;
        t.retain(|r| r.id != 3);
        t.pop();
        t.rollback(sp);
        assert_eq!(t.rows, before);
    }

    #[test]
    fn nested_savepoints() {
        let mut t = table();
        t[0].v = 1;
        let inner = t.savepoint();
        t[0].v = 2;
        t[1].v = 2;
        t.push(R { id: 9, v: 0 });
        t.rollback(inner);
        assert_eq!(t[0].v, 1);
        assert_eq!(t[1].v, 0);
        assert_eq!(t.len(), 4);
    }
}
