#![warn(missing_docs)]

//! A global memory allocator which tries to use huge pages for big allocations

mod mmap;
mod mmapper;

use std::alloc::{handle_alloc_error, GlobalAlloc, Layout, System};
use std::error::Error;
use std::io::Write;
use std::ptr::copy_nonoverlapping;
use std::sync::atomic::{AtomicUsize, Ordering};

use mmapper::MMapper;

/// The global allocator
///
/// To install as the global memory allocator:
///
/// ```rust
/// use huge_global_alloc::HugeGlobalAllocator;
///
/// #[global_allocator]
/// static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(1024 * 1024);
/// ````
pub struct HugeGlobalAllocator {
    mapper: MMapper,
    threshold: AtomicUsize,
}

impl HugeGlobalAllocator {
    /// Creates a new allocator. The threshold defines the minimum number of bytes to consider a
    /// huge page allocation.
    pub const fn new(threshold: usize) -> Self {
        Self {
            mapper: MMapper::new(),
            threshold: AtomicUsize::new(threshold),
        }
    }

    /// Sets the minimum number of bytes to consider a huge page allocation.
    ///
    /// ```rust
    /// use huge_global_alloc::HugeGlobalAllocator;
    ///
    /// #[global_allocator]
    /// static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(0); // Switched off
    /// 
    /// let vec1: Vec<u8> = Vec::with_capacity(1024 * 1024); // 1mb
    /// let stats = GLOBAL_ALLOCATOR.stats().unwrap();
    /// assert_eq!(stats.segments, 0);
    ///
    /// GLOBAL_ALLOCATOR.set_threshold(1024 * 1024); // 1mb
    /// let vec2: Vec<u8> = Vec::with_capacity(1024 * 1024); // 1mb
    /// let stats = GLOBAL_ALLOCATOR.stats().unwrap();
    /// assert_eq!(stats.segments, 1);
    ///
    /// GLOBAL_ALLOCATOR.set_threshold(2 * 1024 * 1024); // 2mb
    /// let vec2: Vec<u8> = Vec::with_capacity(1024 * 1024); // 1mb
    /// let stats = GLOBAL_ALLOCATOR.stats().unwrap();
    /// assert_eq!(stats.segments, 1);
    /// 
    /// ````
    pub fn set_threshold(&self, bytes: usize) {
        self.threshold.store(bytes, Ordering::Relaxed);
    }

    /// Returns allocation statistics from the allocator
    ///
    /// ```rust
    /// use huge_global_alloc::HugeGlobalAllocator;
    ///
    /// #[global_allocator]
    /// static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(1024 * 1024);
    ///
    /// let vec: Vec<u8> = Vec::with_capacity(1024); // 1kb
    /// let stats = GLOBAL_ALLOCATOR.stats().unwrap();
    /// assert_eq!(stats.segments, 0);
    /// assert_eq!(stats.alloc, 0);
    ///
    /// let vec: Vec<u8> = Vec::with_capacity(1024 * 1024); // 1mb
    /// let stats = GLOBAL_ALLOCATOR.stats().unwrap();
    /// assert_eq!(stats.segments, 1);
    /// assert_eq!(stats.alloc, 1024 * 1024);
    /// ````
    pub fn stats(&self) -> Result<HugeGlobalAllocatorStats, Box<dyn Error>> {
        // Gather stats
        self.mapper.stats()
    }

    /// Calls handle_alloc_error with a message and null layout
    fn alloc_error(reason: &'static str) -> ! {
        let layout = unsafe { Layout::from_size_align_unchecked(0, 0) };
        HugeGlobalAllocator::alloc_error_layout(reason, layout)
    }

    /// Calls handle_alloc_error with a message and layout
    fn alloc_error_layout(reason: &'static str, layout: Layout) -> ! {
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
        let threshold = self.threshold.load(Ordering::Relaxed);

        if threshold != 0 && size >= threshold {
            // Allocate the segment
            self.mapper.alloc(layout)
        } else {
            // Revert to system alloc
            System.alloc(layout)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !self.mapper.dealloc(ptr) {
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
            Err(_) => Self::alloc_error_layout("HugeGlobalAllocator::realloc: Failed to create layout", old_layout)
        };

        if self.mapper.is_managed_ptr(old_ptr) {
            // Old ptr is managed
            if new_size >= self.threshold.load(Ordering::Relaxed) {
                // Old ptr is managed and new ptr should be too
                self.mapper.realloc(old_ptr, new_layout)
            } else {
                // Old ptr is managed but new ptr shouldn't be

                // Allocate new segment using the system allocator
                let new_ptr = System.alloc(new_layout);

                if !new_ptr.is_null() {
                    // Copy data from old segment to new
                    copy_nonoverlapping(old_ptr, new_ptr, new_size);
                }

                // Free the old segment
                self.mapper.dealloc(old_ptr);

                new_ptr
            }
        } else {
            // Old ptr is not managed
            if new_size >= self.threshold.load(Ordering::Relaxed) {
                // Old ptr is not managed but new ptr should be

                // Allocate new segment
                let new_ptr = self.mapper.alloc(new_layout);

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
    }
}

/// Allocator performance statistics
#[derive(Debug, Default)]
pub struct HugeGlobalAllocatorStats {
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
    /// Percentage of mapped memory used by allocations
    pub efficiency: usize,
}

#[cfg(test)]
mod tests;
