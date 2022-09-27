#!/bin/bash

# Runs the huge_alloc test with a varying number of free huge pages

function set_huge_pages {
    echo "$1" | sudo tee /proc/sys/vm/nr_hugepages >/dev/null

    if [ $? -ne 0 ];
    then
        echo "Failed to set huge pages"
    fi

    while true
    do
        get_free_huge
        cur_free="$?"

        if [ $cur_free -ne $1 ]
        then
            echo "waiting for required free pages ($cur_free, want $1)"
            sleep 1
        else
            break
        fi
    done
}

function get_free_huge {
    free=$(fgrep "HugePages_Free" /proc/meminfo | awk '{print $2}')

    if [ $? -ne 0 ]
    then
        echo "Failed to get current free huge pages"
        exit 2
    fi

    return $free
}

function run_tests {
    nr_pages=$1

    set_huge_pages $nr_pages

    TEST_NR_PAGES=$nr_pages cargo test --quiet tests::huge_alloc

    if [ $? -ne 0 ]
    then
        exit 3
    fi
}

if [ ! -f /proc/sys/vm/nr_hugepages ];
then
    echo "/proc/sys/vm/nr_hugepages does not exist"
fi

which cargo >/dev/null
if [ $? -ne 0 ]
then
    echo "cargo not found"
fi

orig=$(cat /proc/sys/vm/nr_hugepages)

function cleanup {
    echo "Resetting huge pages to $orig"
    set_huge_pages $orig
}

trap cleanup EXIT

echo "Original nr_hugepages: $orig"

echo "================ Testing with 0 huge pages ================"
run_tests 0

echo "================ Testing with 1 huge page (2mb) ================"
run_tests 1

echo "================ Testing with 2 huge pages (4mb) ================"
run_tests 2

echo "================ Testing with 3 huge pages (6mb) ================"
run_tests 2

echo "================ Testing with 4 huge pages (8mb) ================"
run_tests 4

echo "================ Finished ================"
