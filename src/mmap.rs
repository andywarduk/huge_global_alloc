use std::alloc::Layout;
use std::ffi::c_void;
use std::ptr::null_mut;

use lazy_static::lazy_static;

use nix::sys::mman::{mmap, mremap, munmap, MapFlags, ProtFlags, MRemapFlags};
use nix::unistd::{sysconf, SysconfVar};

use crate::HugeGlobalAllocator;

lazy_static! {
    /// The default page size for the platform
    static ref DEFAULT_PAGE_SIZE: usize = {
        let layout = unsafe { Layout::from_size_align_unchecked(0, 0) };

        match sysconf(SysconfVar::PAGE_SIZE) {
            Ok(val) => match val {
                Some(val) => val as usize,
                None => HugeGlobalAllocator::alloc_error("sysconf PAGE_SIZE no value", layout)
            }
            Err(_) => HugeGlobalAllocator::alloc_error("sysconf PAGE_SIZE failed", layout)
        }
    };
}

/// Descriptor for anonymous memory mapped segments
#[derive(Debug)]
pub struct MMap {
    /// Raw pointer to memory mapped section
    ptr: usize,
    /// Requested layout
    layout: Layout,
    /// Allocation size
    alloc_size: usize,
    /// Page size
    page_size: usize,
}

impl MMap {
    /// Creates a new anonymous memory mapped segment. A huge page allocation is tried initially.
    /// If that fails a default page size allocation is tried.
    pub fn new(layout: Layout) -> nix::Result<MMap> {
        // Try and map a 2mb page size segment first
        match Self::map_2mb(layout) {
            Ok(mmap) => Ok(mmap),
            Err(_) => Self::map_default(layout),
        }
    }

    /// Returns the raw pointer to the memory mapped segment
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr as *mut u8
    }

    /// Returns the allocation size of the segment
    pub fn size(&self) -> usize {
        self.layout.size()
    }

    /// Returns the total mapped size of the segment
    pub fn alloc_size(&self) -> usize {
        self.alloc_size
    }

    /// Returns true if the mapping uses the default page size
    pub fn is_default_page_size(&self) -> bool {
        self.page_size == *DEFAULT_PAGE_SIZE
    }

    /// Remaps a memory section
    pub fn remap(&mut self, new_layout: Layout) -> bool {
        let new_size = new_layout.size();
        let new_alloc_size = Self::calc_alloc_size(new_size, self.page_size);

        let ok = if self.alloc_size != new_alloc_size {
            // Try and remap
            match unsafe { mremap(
                self.ptr as *mut c_void,
                self.alloc_size,
                new_alloc_size,
                MRemapFlags::MREMAP_MAYMOVE,
                None
            ) } {
                Ok(ptr) => {
                    // Success
                    self.ptr = ptr as usize;
                    self.alloc_size = new_alloc_size;

                    true
                }
                Err(_) => {
                    // Failed
                    false
                }
            }
        } else {
            true
        };

        if ok {
            self.layout = new_layout;
        }

        ok
    }

    /// Tries to map an anonymous read write segment with default page size
    fn map_default(layout: Layout) -> nix::Result<MMap> {
        let page_size = *DEFAULT_PAGE_SIZE;
        let alloc_size = Self::calc_alloc_size(layout.size(), page_size);

        let ptr = Self::map_anon(alloc_size, MapFlags::empty())?;

        Ok(MMap {
            ptr: ptr as usize,
            layout,
            alloc_size,
            page_size,
        })
    }
    
    /// Tries to map an anonymous read write segment with 2mb page size
    fn map_2mb(layout: Layout) -> nix::Result<MMap> {
        let page_size = 2 * 1024 * 1024;
        let alloc_size = Self::calc_alloc_size(layout.size(), page_size);

        let ptr = Self::map_anon(alloc_size, MapFlags::MAP_HUGETLB | MapFlags::MAP_HUGE_2MB)?;

        Ok(MMap {
            ptr: ptr as usize,
            layout,
            alloc_size,
            page_size,
        })
    }

    /// Maps an anonymous read write segment with given flags
    fn map_anon(size: usize, flags: MapFlags) -> nix::Result<*mut c_void> {
        let ptr = unsafe {
            mmap(
                null_mut::<c_void>(),
                size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_ANON | MapFlags::MAP_PRIVATE | flags,
                0,
                0,
            )
        }?;

        Ok(ptr)
    }

    /// Calculates the allocation size (whole pages) required for the size required
    fn calc_alloc_size(size: usize, page_size: usize) -> usize {
        (((size - 1) / page_size) + 1) * page_size
    }
}

impl Drop for MMap {
    /// Unmaps the anonymous memory mapped segment on drop
    fn drop(&mut self) {
        let size = self.alloc_size();

        if unsafe { munmap(self.ptr as *mut c_void, size) }.is_err() {
            HugeGlobalAllocator::alloc_error("MMapper::realloc: failed to unmap", self.layout);
        }
    }
}
