//! Net heap growth while each kind of asset body is being loaded (iOS memory hunt).
//!
//! `begin()` notes this thread's net allocated bytes; `end(kind, mark)` adds the
//! difference to that kind's total. Only allocations made by the calling thread are
//! counted, so parallel loads on other threads do not leak in. It is a net figure:
//! an asset that frees a buffer another thread allocated earlier shows less.
//!
//! Two numbers per kind: `total` includes assets nested inside it (a weapon owns its
//! models and animations), `own` subtracts what its nested assets grew by. `own` is
//! what the kind itself costs. Read both as a ranking, not as exact sizes.
//! Zone decompression buffers are allocated outside any asset body and do not appear.
//!
//! The per-thread counter lives in the global allocator and is always on (it does not
//! depend on `IW4L_COUNTING_ALLOC`); the launcher installs that allocator on every platform.
use std::cell::RefCell;
use std::sync::Mutex;

use crate::alloc_count;

#[derive(Clone, Copy)]
struct Row {
    kind: &'static str,
    count: u64,
    net: i64,
    own: i64,
}

static ROWS: Mutex<Vec<Row>> = Mutex::new(Vec::new());

thread_local! {
    /// Growth already claimed by finished child assets, one slot per open `begin()`.
    static CHILD_NET: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

/// Opaque marker taken at the start of an asset body.
#[derive(Clone, Copy)]
pub struct Mark {
    net: i64,
    depth: usize,
}

pub fn begin() -> Mark {
    let depth = CHILD_NET
        .try_with(|stack| {
            let mut stack = stack.borrow_mut();
            stack.push(0);
            stack.len() - 1
        })
        .unwrap_or(0);
    Mark { net: alloc_count::thread_net_bytes(), depth }
}

pub fn end(kind: &'static str, mark: Mark) {
    let (grown, own) = settle(mark);
    let Ok(mut rows) = ROWS.lock() else { return };
    match rows.iter_mut().find(|row| row.kind == kind) {
        Some(row) => {
            row.count += 1;
            row.net += grown;
            row.own += own;
        }
        None => rows.push(Row { kind, count: 1, net: grown, own }),
    }
}

/// Closes one asset body: returns (total growth, growth of the asset itself) and
/// credits the total to the asset that encloses it.
fn settle(mark: Mark) -> (i64, i64) {
    let grown = alloc_count::thread_net_bytes() - mark.net;
    // A load that bailed out with `?` never called `end`: drop its orphaned slots.
    let children = CHILD_NET
        .try_with(|stack| {
            let mut stack = stack.borrow_mut();
            stack.truncate(mark.depth + 1);
            let children = stack.pop().unwrap_or(0);
            if let Some(parent) = stack.last_mut() {
                *parent += grown;
            }
            children
        })
        .unwrap_or(0);
    (grown, grown - children)
}

/// One line, largest total growth first, empty when nothing was recorded.
pub fn report() -> String {
    let Ok(rows) = ROWS.lock() else { return String::new() };
    if rows.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<Row> = rows.clone();
    sorted.sort_by(|a, b| b.net.cmp(&a.net));
    let mib = |bytes: i64| bytes / (1024 * 1024);
    let parts: Vec<String> = sorted
        .iter()
        .take(24)
        .map(|row| {
            format!(
                "{} {}x total {}MiB own {}MiB",
                row.kind,
                row.count,
                mib(row.net),
                mib(row.own)
            )
        })
        .collect();
    format!(
        "heap net growth per asset kind (top 10, per-thread net; total includes nested assets, own does not): {}",
        parts.join(" | ")
    )
}

/// Scope guard: records on drop, so early `return`/`?` paths are still counted.
pub struct Scope {
    kind: &'static str,
    mark: Mark,
}

pub fn scope(kind: &'static str) -> Scope {
    Scope { kind, mark: begin() }
}

impl Drop for Scope {
    fn drop(&mut self) {
        end(self.kind, self.mark);
    }
}
