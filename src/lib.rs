#![warn(missing_docs)]

//! A global memory allocator which tries to use huge pages for big allocations

mod mmap;
mod mmapper;

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::io::Write;
use std::ptr::copy_nonoverlapping;
use std::sync::Mutex;

use mmapper::{MMapper, MMapperStats};

/// The global allocator
/// 
/// To use as the global memory allocator:
/// 
/// ```rust
/// use huge_global_alloc::HugeGlobalAllocator;
///
/// #[global_allocator]
/// static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(1024 * 1024);
/// ````
pub struct HugeGlobalAllocator {
    mapper: Mutex<MMapper>,
    threshold: usize,
}

impl HugeGlobalAllocator {
    /// Creates a new allocator. The threshold defines the minimum number of bytes to consider a
    /// huge page allocation.
    pub const fn new(threshold: usize) -> Self {
        assert!(threshold >= 1024 * 1024);

        Self {
            mapper: Mutex::new(MMapper::new()),
            threshold,
        }
    }

    /// Gets allocation statistics
    pub fn stats(&self) -> MMapperStats {
        // Lock the mapper
        if let Ok(mapper) = self.mapper.lock().as_ref() {
            // Gather stats
            mapper.stats()
        } else {
            panic!("HugeGlobalAllocator::dealloc: Failed to lock the mapper");
        }
    }

    fn alloc_error(reason: &'static str, layout: Layout) -> ! {
        unsafe {
            std::io::stderr().write(reason.as_bytes()).unwrap_unchecked();
            std::io::stderr().write("\n".as_bytes()).unwrap_unchecked();
        }

        handle_alloc_error(layout);
    }
}

unsafe impl GlobalAlloc for HugeGlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();

        if size >= self.threshold {
            // Lock the mapper
            if let Ok(mapper) = self.mapper.lock().as_mut() {
                // Allocate the segment
                mapper.alloc(layout)
            } else {
                Self::alloc_error("HugeGlobalAllocator::alloc: Failed to lock the mapper", layout);
            }
        } else {
            // Revert to system alloc
            System.alloc(layout)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Lock the mapper
        let dealloced = if let Ok(mapper) = self.mapper.lock().as_mut() {
            // Dellocate the segment (if it's a mapped segment)
            mapper.dealloc(ptr)
        } else {
            Self::alloc_error("HugeGlobalAllocator::dealloc: Failed to lock the mapper", layout);
        };

        if !dealloced {
            // Revert to system dealloc
            System.dealloc(ptr, layout)
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // Anonymous mem maps are zeroed already
        self.alloc(layout)
    }

    unsafe fn realloc(&self, old_ptr: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
        // Create new layout
        let new_layout = match Layout::from_size_align(new_size, old_layout.align()) {
            Ok(layout) => layout,
            Err(_) => Self::alloc_error("HugeGlobalAllocator::realloc: Failed to create layout", old_layout)
        };

        // Lock the mapper
        if let Ok(mapper) = self.mapper.lock().as_mut() {
            if mapper.is_managed_ptr(old_ptr) {
                // Old ptr is managed
                if new_size >= self.threshold {
                    // Old ptr is managed and new ptr should be too
                    mapper.realloc(old_ptr, new_layout)
                } else {
                    // Old ptr is managed but new ptr shouldn't be

                    // Allocate new segment using the system allocator
                    let new_ptr = System.alloc(new_layout);

                    if !new_ptr.is_null() {
                        // Copy data from old segment to new
                        copy_nonoverlapping(old_ptr, new_ptr, new_size);
                    }

                    // Free the old segment
                    mapper.dealloc(old_ptr);

                    new_ptr
                }
            } else {
                // Old ptr is not managed
                if new_size >= self.threshold {
                    // Old ptr is not managed but new ptr should be

                    // Allocate new segment
                    let new_ptr = mapper.alloc(new_layout);

                    if !new_ptr.is_null() {
                        // Copy data from old segment to new
                        copy_nonoverlapping(old_ptr, new_ptr, old_layout.size());
                    }

                    // Free the old segment
                    System.dealloc(old_ptr, old_layout);

                    new_ptr
                } else {
                    // Old ptr is not managed and new ptr shouldn't be - revert to system realloc
                    System.realloc(old_ptr, old_layout, new_size)
                }
            }
        } else {
            Self::alloc_error("HugeGlobalAllocator::realloc: Failed to lock the mapper", old_layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[global_allocator]
    static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(1024 * 1024);

    fn mb(mb: usize) -> usize {
        mb * 1024 * 1024
    }

    fn check_stats(desc: &str, expected_segs: usize, expected_mapped: usize) -> MMapperStats {
        let stats = GLOBAL_ALLOCATOR.stats();

        println!("{}: {:?}", desc, stats);

        assert_eq!(expected_segs, stats.segments, "{} segments", desc);

        let avail_bytes = if let Ok(env) = std::env::var("TEST_NR_PAGES") {
            let avail_pages = env.parse::<usize>().expect("TEST_NR_PAGES not numeric");
            avail_pages * mb(2)
        } else {
            0
        };

        if avail_bytes >= mb(6) {
            // Enough huge pages to satisfy
            assert_eq!(expected_mapped, stats.mapped, "{} mapped", desc);
            assert_eq!(expected_mapped, stats.huge_mapped, "{} huge mapped", desc);
            assert_eq!(0, stats.default_mapped, "{} default mapped", desc);
        } else if stats.huge_segments > 0 {
            assert_eq!(expected_mapped, stats.mapped, "{} mapped", desc);
        } else {
            assert!(stats.mapped >= stats.alloc, "{} mapped >= alloc", desc);
        }

        assert_eq!(stats.default_segments + stats.huge_segments, stats.segments, "{} segment sum", desc);
        assert_eq!(stats.default_mapped + stats.huge_mapped, stats.mapped, "{} mapped sum", desc);
        assert_eq!(stats.default_alloc + stats.huge_alloc, stats.alloc, "{} alloc sum", desc);

        stats
    }

    fn check_stats_eq(desc: &str, expected_alloc: usize, expected_segs: usize, expected_mapped: usize) {
        let stats = check_stats(desc, expected_segs, expected_mapped);
        assert_eq!(expected_alloc, stats.alloc, "{} alloc", desc);
    }

    fn check_stats_gt(desc: &str, expected_alloc: usize, expected_segs: usize, expected_mapped: usize) {
        let stats = check_stats(desc, expected_segs, expected_mapped);
        assert!(stats.alloc > expected_alloc, "{} alloc", desc);
    }

    fn check_stats_ge(desc: &str, expected_alloc: usize, expected_segs: usize, expected_mapped: usize) {
        let stats = check_stats(desc, expected_segs, expected_mapped);
        assert!(stats.alloc >= expected_alloc, "{} alloc", desc);
    }

    fn check_stats_lt(desc: &str, expected_alloc: usize, expected_segs: usize, expected_mapped: usize) {
        let stats = check_stats(desc, expected_segs, expected_mapped);
        assert!(stats.alloc < expected_alloc, "{} alloc", desc);
    }

    #[test]
    fn huge_alloc() {
        let mut vec = Vec::new();

        // 512 * 1024 * 8 = 4mb
        let items = 512 * 1024;

        let vec_mb = |items| {
            let bytes = items * 8;

            if bytes % mb(1) == 0 {
                Some(bytes / mb(1))
            } else {
                None
            }
        };

        for i in 0..items {
            let on_mb = vec_mb(i);

            if let Some(cur) = on_mb {
                match cur {
                    0 => check_stats_eq("initial", 0, 0, 0),
                    1 => check_stats_ge(">= 1mb", mb(cur), 1, mb(2)),
                    2 => check_stats_ge(">= 2mb", mb(cur), 1, mb(2)),
                    3 => check_stats_ge(">= 3mb", mb(cur), 1, mb(4)),
                    _ => panic!("mb boundary not handled")
                }
            }

            vec.push(i);

            if let Some(cur) = on_mb {
                match cur {
                    0 => check_stats_eq("> 0", 0, 0, 0),
                    1 => check_stats_gt("> 1mb", mb(cur), 1, mb(2)),
                    2 => check_stats_gt("> 2mb", mb(cur), 1, mb(4)),
                    3 => check_stats_gt("> 3mb", mb(cur), 1, mb(4)),
                    _ => panic!("mb boundary not handled")
                }
            }
        }

        assert_eq!(vec.len(), items, "vector entries incorrect");

        println!("Popping {} items ({} bytes)", items, items * 8);

        for i in (0..items).rev() {
            vec.pop().unwrap();

            assert_eq!(i, vec.len());

            if let Some(cur) =  vec_mb(i + 1) {
                vec.shrink_to_fit();

                assert_eq!(i, vec.capacity());

                match cur {
                    0 => (),
                    1 => check_stats_eq("< 1mb", 0, 0, 0),
                    2 => check_stats_lt("< 2mb", mb(cur), 1, mb(2)),
                    3 => check_stats_lt("< 3mb", mb(cur), 1, mb(4)),
                    4 => check_stats_lt("< 4mb", mb(cur), 1, mb(4)),
                    _ => panic!("mb boundary not handled")
                }
            }
        }
    }
}
