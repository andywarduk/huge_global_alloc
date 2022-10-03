# huge_globale_alloc

A huge page global allocator for rust.

This global allocator for rust will try and map 2mb anonymous huge pages for large allocations when the requested size is above a threshold. If the allocation fails due to insufficient available huge pages, default page size pages are mapped instead. Falls back to using the System allocator for small allocations.

## Example usage

To create the global allocator with a size threshold of 1 mb:

```rust
#[global_allocator]
static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(1024 * 1024);
```

Statistics can be retrieved from the allocator with stats():

```rust
let stats = GLOBAL_ALLOCATOR.stats().unwrap();
```

## Huge page configuration

To enable huge pages (eg. 20 2 mb pages reserved):

```sh
echo 20 | sudo tee /proc/sys/vm/nr_hugepages >/dev/null
```

Huge page stats can be queried from /proc/meminfo:

```sh
$ fgrep -i huge /proc/meminfo
AnonHugePages:         0 kB
ShmemHugePages:        0 kB
FileHugePages:         0 kB
HugePages_Total:     331
HugePages_Free:      331
HugePages_Rsvd:        0
HugePages_Surp:        0
Hugepagesize:       2048 kB
Hugetlb:          677888 kB
```

Configuration can also be made through the newer sysfs filesystem:

```sh
$ ls /sys/kernel/mm/hugepages/
hugepages-1048576kB  hugepages-2048kB

$ ls -l /sys/kernel/mm/hugepages/*
/sys/kernel/mm/hugepages/hugepages-1048576kB:
total 0
-r--r--r-- 1 root root 4096 Sep 27 18:41 free_hugepages
-rw-r--r-- 1 root root 4096 Sep 27 18:41 nr_hugepages
-rw-r--r-- 1 root root 4096 Sep 27 18:41 nr_hugepages_mempolicy
-rw-r--r-- 1 root root 4096 Sep 27 18:41 nr_overcommit_hugepages
-r--r--r-- 1 root root 4096 Sep 27 18:41 resv_hugepages
-r--r--r-- 1 root root 4096 Sep 27 18:41 surplus_hugepages

/sys/kernel/mm/hugepages/hugepages-2048kB:
total 0
-r--r--r-- 1 root root 4096 Sep 27 18:41 free_hugepages
-rw-r--r-- 1 root root 4096 Sep 27 18:41 nr_hugepages
-rw-r--r-- 1 root root 4096 Sep 27 18:41 nr_hugepages_mempolicy
-rw-r--r-- 1 root root 4096 Sep 27 18:41 nr_overcommit_hugepages
-r--r--r-- 1 root root 4096 Sep 27 18:41 resv_hugepages
-r--r--r-- 1 root root 4096 Sep 27 18:41 surplus_hugepages
```
