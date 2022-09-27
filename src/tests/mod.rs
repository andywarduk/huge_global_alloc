use super::*;

#[global_allocator]
static GLOBAL_ALLOCATOR: HugeGlobalAllocator = HugeGlobalAllocator::new(1024 * 1024);

fn mb(mb: usize) -> usize {
    mb * 1024 * 1024
}

fn check_stats(desc: &str, expected_segs: usize, expected_mapped: usize) -> HugeGlobalAllocatorStats {
    let stats = GLOBAL_ALLOCATOR.stats().unwrap();

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
                _ => panic!("mb boundary not handled"),
            }
        }

        vec.push(i);

        if let Some(cur) = on_mb {
            match cur {
                0 => check_stats_eq("> 0", 0, 0, 0),
                1 => check_stats_gt("> 1mb", mb(cur), 1, mb(2)),
                2 => check_stats_gt("> 2mb", mb(cur), 1, mb(4)),
                3 => check_stats_gt("> 3mb", mb(cur), 1, mb(4)),
                _ => panic!("mb boundary not handled"),
            }
        }
    }

    assert_eq!(vec.len(), items, "vector entries incorrect");

    println!("Popping {} items ({} bytes)", items, items * 8);

    for i in (0..items).rev() {
        vec.pop().unwrap();

        assert_eq!(i, vec.len());

        if let Some(cur) = vec_mb(i + 1) {
            vec.shrink_to_fit();

            assert_eq!(i, vec.capacity());

            match cur {
                0 => (),
                1 => check_stats_eq("< 1mb", 0, 0, 0),
                2 => check_stats_lt("< 2mb", mb(cur), 1, mb(2)),
                3 => check_stats_lt("< 3mb", mb(cur), 1, mb(4)),
                4 => check_stats_lt("< 4mb", mb(cur), 1, mb(4)),
                _ => panic!("mb boundary not handled"),
            }
        }
    }
}
