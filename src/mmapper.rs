use std::{
    alloc::Layout,
    collections::HashMap,
    error::Error,
    ptr::copy_nonoverlapping,
    sync::{Mutex, MutexGuard},
};

use crate::{mmap::MMap, HugeGlobalAllocator, HugeGlobalAllocatorStats};

/// A collection of tracked memory mapped segments
pub struct MMapper {
    ptr_map: Mutex<Option<HashMap<usize, MMap>>>,
    stats: Mutex<MMapperStats>,
}

impl MMapper {
    /// Create a new memory mappings container
    pub const fn new() -> Self {
        Self {
            ptr_map: Mutex::new(None),
            stats: Mutex::new(MMapperStats::new()),
        }
    }

    /// Allocates an anonymous memory mapped segment
    pub fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();

        // Create the anon memory map
        let mmap = match MMap::new(layout) {
            Ok(mmap) => mmap,
            Err(_) => HugeGlobalAllocator::alloc_error_layout("MMapper::alloc: failed to map segment", layout)
        };

        if mmap.is_default_page_size() {
            // Log missed allocation
            self.add_missed(size);
        }

        // Get raw pointer
        let ptr = mmap.as_ptr();

        // Insert in to hash map
        self.map_add(mmap);

        ptr
    }

    /// Deallocates an anonymous memory mapped segment
    pub fn dealloc(&self, ptr: *mut u8) -> bool {
        // Remove from the map
        self.map_remove(ptr).is_some()
    }

    /// Reallocates an anonymous memory mapped segment
    pub fn realloc(&self, ptr: *mut u8, layout: Layout) -> *mut u8 {
        let new_size = layout.size();

        // Remove existing map entry
        if let Some(mut mmap) = self.map_remove(ptr) {
            let was_default = mmap.is_default_page_size();
            let old_size = mmap.size();

            // Do the reallocate
            if mmap.remap(layout) {
                // Get raw pointer
                let ptr = mmap.as_ptr();

                if was_default {
                    // Was default
                    if new_size > old_size {
                        // Add extra space as missed
                        self.add_missed(new_size - old_size);
                    }
                } else if mmap.is_default_page_size() {
                    // Was huge and is now not
                    self.add_missed(new_size);
                }

                // Insert it back in to the hash map
                self.map_add(mmap);

                ptr
            } else {
                // Failed to remap
                let mut stats = self.lock_stats();

                stats.remaps_failed += 1;

                drop(stats);

                // Allocate new segment
                let new_ptr = self.alloc(layout);

                // Copy data from old segment to new
                unsafe {
                    copy_nonoverlapping(mmap.as_ptr(), new_ptr, old_size);
                }

                new_ptr
            }
        } else {
            HugeGlobalAllocator::alloc_error_layout("MMapper::realloc: ptr not found", layout);
        }
    }

    /// Returns statistics for the mapper
    pub(crate) fn stats(&self) -> Result<HugeGlobalAllocatorStats, Box<dyn Error>> {
        let mut out_stats = HugeGlobalAllocatorStats::default();

        // Lock the ptr_map
        if let Some(ptr_map) = self.lock_map().as_ref() {
            for mmap in ptr_map.values() {
                out_stats.alloc += mmap.size();
                out_stats.mapped += mmap.alloc_size();
                out_stats.segments += 1;

                if mmap.is_default_page_size() {
                    out_stats.default_alloc += mmap.size();
                    out_stats.default_mapped += mmap.alloc_size();
                    out_stats.default_segments += 1;
                } else {
                    out_stats.huge_alloc += mmap.size();
                    out_stats.huge_mapped += mmap.alloc_size();
                    out_stats.huge_segments += 1;
                }
            }
        }

        let stats = self.lock_stats();

        out_stats.missed_allocs = stats.missed_allocs;
        out_stats.missed_mb = stats.missed_mb as f64 + (stats.missed_bytes as f64 / (1024 * 1024) as f64);
        out_stats.remaps_failed = stats.remaps_failed;

        drop(stats);

        if out_stats.mapped == 0 {
            out_stats.efficiency = 100;
        } else {
            out_stats.efficiency = (out_stats.alloc * 100) / out_stats.mapped;
        }

        Ok(out_stats)
    }

    /// Returns true if the passed pointer is managed by the mapper
    pub(crate) fn is_managed_ptr(&self, ptr: *mut u8) -> bool {
        // Lock the ptr_map
        if let Some(ptr_map) = self.lock_map().as_ref() {
            // Look for map entry
            ptr_map.contains_key(&(ptr as usize))
        } else {
            // ptr_map does not exist
            false
        }
    }

    /// Removes an entry from the pointer map
    fn map_remove(&self, ptr: *mut u8) -> Option<MMap> {
        // Lock the ptr_map
        if let Some(ptr_map) = self.lock_map().as_mut() {
            // Remove map entry
            ptr_map.remove(&(ptr as usize))
        } else {
            // ptr_map does not exist
            None
        }
    }

    /// Adds an entry from the pointer map
    fn map_add(&self, mmap: MMap) {
        let layout = mmap.layout();

        // Lock the ptr_map
        let mut lock = self.lock_map_for_insert();
        let ptr_map = lock.as_mut().unwrap();

        // Add map entry
        if ptr_map.insert(mmap.ptr(), mmap).is_some() {
            HugeGlobalAllocator::alloc_error_layout("MMapper::map_add: map already exists", layout);
        }
    }

    /// Locks the ptr_map for insertion, creating if necessary
    fn lock_map_for_insert(&self) -> MutexGuard<Option<HashMap<usize, MMap>>> {
        let mut map = self.lock_map();

        if map.is_none() {
            *map = Some(HashMap::new());
        }

        map
    }

    /// Locks the ptr_map for removal
    fn lock_map(&self) -> MutexGuard<Option<HashMap<usize, MMap>>> {
        // Lock the ptr_map
        match self.ptr_map.lock() {
            Ok(ptr_map) => ptr_map,
            _ => HugeGlobalAllocator::alloc_error("MMapper::lock_map: unable to lock ptr_map"),
        }
    }

    /// Locks statistics
    fn lock_stats(&self) -> MutexGuard<MMapperStats> {
        // Lock stats
        match self.stats.lock() {
            Ok(stats) => stats,
            _ => HugeGlobalAllocator::alloc_error("MMapper::lock_stats: unable to lock stats"),
        }
    }

    /// Add statistics about missed huge allocations
    fn add_missed(&self, bytes: usize) {
        let mut stats = self.lock_stats();

        stats.missed_allocs += 1;

        stats.missed_bytes += bytes;

        if stats.missed_bytes > (1024 * 1024) {
            let mb = stats.missed_bytes / (1024 * 1024);
            stats.missed_bytes -= mb * (1024 * 1024);
            stats.missed_mb += mb;
        }
    }
}

#[derive(Default)]
struct MMapperStats {
    missed_allocs: usize,
    missed_bytes: usize,
    missed_mb: usize,
    remaps_failed: usize,
}

impl MMapperStats {
    const fn new() -> Self {
        Self {
            missed_allocs: 0,
            missed_bytes: 0,
            missed_mb: 0,
            remaps_failed: 0,
        }
    }
}
