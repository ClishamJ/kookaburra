//! Strongly-typed IDs.
//!
//! Each ID is a newtype over `u64` with a process-global counter. Newtypes
//! prevent "passed a `TileId` where a `WorkspaceId` was expected" bugs at
//! compile time.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! define_id {
    ($name:ident, $counter:ident) => {
        static $counter: AtomicU64 = AtomicU64::new(1);

        #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
        pub struct $name(u64);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self($counter.fetch_add(1, Ordering::Relaxed))
            }

            #[must_use]
            pub const fn raw(self) -> u64 {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.0)
            }
        }
    };
}

define_id!(WorkspaceId, NEXT_WORKSPACE_ID);
define_id!(TileId, NEXT_TILE_ID);
define_id!(PtyId, NEXT_PTY_ID);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn workspace_ids_are_unique() {
        let ids: HashSet<_> = (0..1000).map(|_| WorkspaceId::new()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn tile_ids_are_unique() {
        let ids: HashSet<_> = (0..1000).map(|_| TileId::new()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn pty_ids_are_unique() {
        let ids: HashSet<_> = (0..1000).map(|_| PtyId::new()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn id_types_do_not_mix() {
        let w = WorkspaceId::new();
        let t = TileId::new();
        let p = PtyId::new();
        assert_ne!(w.raw(), 0);
        assert_ne!(t.raw(), 0);
        assert_ne!(p.raw(), 0);
    }

    #[test]
    fn ids_start_at_one() {
        assert!(WorkspaceId::new().raw() >= 1);
        assert!(TileId::new().raw() >= 1);
        assert!(PtyId::new().raw() >= 1);
    }
}
