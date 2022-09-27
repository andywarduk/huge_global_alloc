use std::{collections::HashMap, ptr::copy_nonoverlapping, alloc::Layout};

use crate::{mmap::MMap, HugeGlobalAllocator};

/// A collection of tracked memory mapped segments
pub struct MMapper {
    ptr_map: Option<HashMap<usize, MMap>>,
    missed_allocs: usize,
    missed_bytes: usize,
    missed_mb: usize,
    remaps_failed: usize,
}

impl MMapper {
    /// Create a new memory mappings container
    pub const fn new() -> Self {
        Self {
            ptr_map: None,
            missed_allocs: 0,
            missed_bytes: 0,
            missed_mb: 0,
            remaps_failed: 0,
        }
    }

    /// Allocates an anonymous memory mapped segment
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size();

        // Create the anon memory map
        let mmap = match MMap::new(layout) {
            Ok(mmap) => mmap,
            Err(_) => HugeGlobalAllocator::alloc_error("MMapper::alloc: failed to map segment", layout)
        };

        if mmap.is_default_page_size() {
            self.add_missed(size);
        }

        // Get raw pointer
        let ptr = mmap.as_ptr();

        // Insert in to hash map
        if self.ptr_map().insert(ptr as usize, mmap).is_some() {
            HugeGlobalAllocator::alloc_error("MMapper::alloc: address already mapped", layout);
        }

        ptr
    }

    /// Deallocates an anonymous memory mapped segment
    pub fn dealloc(&mut self, ptr: *mut u8) -> bool {
        if let Some(ptr_map) = self.ptr_map.as_mut() {
            // Remove map entry
            ptr_map.remove(&(ptr as usize)).is_some()
        } else {
            false
        }
    }

    /// Reallocates an anonymous memory mapped segment
    pub fn realloc(&mut self, ptr: *mut u8, layout: Layout) -> *mut u8 {
        let new_size = layout.size();

        if let Some(ptr_map) = self.ptr_map.as_mut() {
            // Remove existing map entry
            if let Some(mut mmap) = ptr_map.remove(&(ptr as usize)) {
                let was_default = mmap.is_default_page_size();
                let old_size = mmap.size();

                // Do the reallocate
                if mmap.remap(layout) {
                    // Get raw pointer
                    let ptr = mmap.as_ptr();

                    if was_default {
                        // Was default
                        if new_size > old_size {
                            self.add_missed(new_size - old_size);
                        }
                    } else if mmap.is_default_page_size() {
                        // Was huge and is not not
                        self.add_missed(new_size);
                    }

                    // Insert in to hash map
                    if self.ptr_map().insert(ptr as usize, mmap).is_some() {
                        HugeGlobalAllocator::alloc_error("MMapper::realloc: address already mapped", layout);
                    }

                    ptr
                } else {
                    // Failed to remap
                    self.remaps_failed += 1;

                    // Allocate new segment
                    let new_ptr = self.alloc(layout);

                    // Copy data from old segment to new
                    unsafe {
                        copy_nonoverlapping(mmap.as_ptr(), new_ptr, old_size);
                    }

                    new_ptr
                }
            } else {
                HugeGlobalAllocator::alloc_error("MMapper::realloc: ptr not found", layout);
            }
        } else {
            HugeGlobalAllocator::alloc_error("MMapper::realloc: no ptr_map trying to remove", layout);
        }
    }

    /// Returns statistics for the mapper
    pub(crate) fn stats(&self) -> MMapperStats {
        let mut stats = MMapperStats::default();

        if let Some(ptr_map) = self.ptr_map.as_ref() {
            for mmap in ptr_map.values() {
                stats.alloc += mmap.size();
                stats.mapped += mmap.alloc_size();
                stats.segments += 1;

                if mmap.is_default_page_size() {
                    stats.default_alloc += mmap.size();
                    stats.default_mapped += mmap.alloc_size();
                    stats.default_segments += 1;
                } else {
                    stats.huge_alloc += mmap.size();
                    stats.huge_mapped += mmap.alloc_size();
                    stats.huge_segments += 1;
                }
            }
        }

        stats.missed_allocs = self.missed_allocs;
        stats.missed_mb = self.missed_mb as f64 + (self.missed_bytes as f64 / (1024 * 1024) as f64);
        stats.remaps_failed = self.remaps_failed;

        stats
    }

    /// Returns true if the passed pointer is managed by the mapper
    pub(crate) fn is_managed_ptr(&self, ptr: *mut u8) -> bool {
        if let Some(ptr_map) = self.ptr_map.as_ref() {
            ptr_map.contains_key(&(ptr as usize))
        } else {
            false
        }
    }

    /// Gets the pointer map creating it if necessary
    fn ptr_map(&mut self) -> &mut HashMap<usize, MMap> {
        if self.ptr_map.is_none() {
            self.ptr_map = Some(HashMap::new());
        }

        // This unwrap is safe as we just created it
        self.ptr_map.as_mut().unwrap()
    }

    /// Add statistics about missed huge allocations
    fn add_missed(&mut self, bytes: usize) {
        self.missed_allocs += 1;

        self.missed_bytes += bytes;

        if self.missed_bytes > (1024 * 1024) {
            let mb = self.missed_bytes / (1024 * 1024);
            self.missed_bytes -= mb * (1024 * 1024);
            self.missed_mb += mb;
        }
    }
}

#[derive(Debug, Default)]
pub struct MMapperStats {
    /// Total amount of memory allocated in bytes
    pub alloc: usize,
    /// Total amount of memory mapped in bytes
    pub mapped: usize,
    /// Total number of segments mapped
    pub segments: usize,

    /// Amount of memory allocated in default page size pages in bytes
    pub default_alloc: usize,
    /// Amount of memory mapped in default page size pages in bytes
    pub default_mapped: usize,
    /// Number of default page size segments mapped
    pub default_segments: usize,

    /// Amount of memory allocated in huge pages in bytes
    pub huge_alloc: usize,
    /// Amount of memory mapped in huge pages in bytes
    pub huge_mapped: usize,
    /// Number of huge page segments mapped
    pub huge_segments: usize,

    /// Number of allocations missed due to lack of huge pages
    pub missed_allocs: usize,
    /// Allocations missed due to lack of huge pages in total megabytes
    pub missed_mb: f64,
    /// Number of failed remaps
    pub remaps_failed: usize,
}
