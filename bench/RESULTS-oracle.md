# ORM benchmark results: oracle

3 rounds, timings pooled, memory as the median of the rounds. 2026-10-13, Linux 7.0.0-34-generic, rustc 1.96.1 (31fca3adb 2026-06-26), PHP 8.5.10, Oracle 21.3.0.0.0; diesel through diesel-oci, Eloquent through yajra/laravel-oci8.

## Summary: mean time per operation (fastest in bold)

| Scenario | diesel | eloquent-opcache-jit | eloquent | rustique |
|---|---:|---:|---:|---:|
| bulk_insert_1000 | **73.9 ms** | 95.3 ms | 155.6 ms | 138.5 ms |
| bulk_insert_10000 | 1,220.9 ms | 7,211.0 ms | 8,013.6 ms | **1,219.2 ms** |
| eager_1000_users_10000_posts | 67.7 ms | 2,693.0 ms | 2,705.4 ms | **48.9 ms** |
| eager_50_users_500_posts | 3.6 ms | 136.3 ms | 137.8 ms | **3.1 ms** |
| fetch_100000_rows | 325.2 ms | 909.6 ms | 1,072.0 ms | **285.2 ms** |
| fetch_10000_rows | 34.8 ms | 83.8 ms | 113.8 ms | **29.8 ms** |
| fetch_1000_rows | **3.2 ms** | 8.3 ms | 9.6 ms | 3.5 ms |
| find_by_pk | **153 µs** | 230 µs | 231 µs | 192 µs |
| insert_single | 605 µs | 652 µs | 801 µs | **601 µs** |
| json_fetch_1000 | **5.2 ms** | 273.1 ms | 265.6 ms | 5.4 ms |
| json_fetch_10000 | **55.4 ms** | 2,655.8 ms | 2,859.9 ms | 55.5 ms |
| json_insert_single | 1.2 ms | 701 µs | 734 µs | **609 µs** |
| json_update_single | 636 µs | 1.6 ms | 994 µs | **629 µs** |
| json_where_limit_20 | 537 µs | 6.0 ms | 6.1 ms | **514 µs** |
| update_single | 564 µs | 650 µs | 718 µs | **554 µs** |
| where_order_limit_20 | 571 µs | 701 µs | 671 µs | **557 µs** |

## Summary: memory

Heap peak: most memory in use above the start of the scenario, per allocator (Rust: global allocator; PHP: `memory_get_peak_usage()`). It misses what C client libraries take with `malloc`: libmysqlclient under diesel on MySQL, and Oracle's OCI under both Rust ORMs on Oracle. Peak RSS: whole process, highest during the scenario.

| Scenario | diesel heap MB | eloquent-opcache-jit heap MB | eloquent heap MB | rustique heap MB | diesel RSS MB | eloquent-opcache-jit RSS MB | eloquent RSS MB | rustique RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| bulk_insert_1000 | 0.1 | 2.4 | 2.4 | 0.3 | 566.6 | 261.7 | 247.7 | 70.0 |
| bulk_insert_10000 | 1.5 | 22.8 | 22.8 | 3.2 | 566.6 | 302.8 | 288.0 | 70.1 |
| eager_1000_users_10000_posts | 8.4 | 17.9 | 17.9 | 11.3 | 530.1 | 240.6 | 230.3 | 57.6 |
| eager_50_users_500_posts | 0.4 | 1.1 | 0.9 | 0.5 | 382.6 | 239.2 | 228.9 | 43.5 |
| fetch_100000_rows | 78.4 | 148.2 | 148.2 | 51.2 | 382.4 | 238.6 | 229.0 | 94.1 |
| fetch_10000_rows | 8.0 | 14.9 | 14.9 | 5.8 | 71.9 | 103.5 | 93.8 | 45.4 |
| fetch_1000_rows | 0.8 | 1.5 | 1.5 | 0.5 | 41.2 | 89.5 | 79.6 | 39.5 |
| find_by_pk | 0.1 | 0.4 | 0.0 | 0.1 | 37.6 | 88.5 | 78.4 | 39.1 |
| insert_single | 0.0 | 0.1 | 0.0 | 0.0 | 566.6 | 258.6 | 244.0 | 70.0 |
| json_fetch_1000 | 0.7 | 2.1 | 2.1 | 0.9 | 530.3 | 241.1 | 230.9 | 60.4 |
| json_fetch_10000 | 6.7 | 20.8 | 20.8 | 11.0 | 566.5 | 250.1 | 240.0 | 69.9 |
| json_insert_single | 0.0 | 0.0 | 0.0 | 0.0 | 566.6 | 258.6 | 244.0 | 70.0 |
| json_update_single | 0.0 | 1.1 | 1.1 | 0.0 | 566.6 | 257.7 | 244.0 | 70.0 |
| json_where_limit_20 | 0.0 | 0.2 | 0.0 | 0.0 | 566.5 | 250.6 | 240.0 | 69.9 |
| update_single | 0.0 | 0.9 | 0.9 | 0.0 | 566.6 | 257.5 | 243.6 | 70.0 |
| where_order_limit_20 | 0.0 | 0.1 | 0.0 | 0.0 | 37.7 | 88.6 | 78.5 | 39.2 |

## Per scenario

### bulk_insert_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 60 | 73.9 | 13.9 | 17.0 | 2,341.3 | 13.5 | 1.00x | 0.1 | 566.6 |
| eloquent-opcache-jit | 60 | 95.3 | 80.6 | 93.5 | 916.1 | 10.5 | 1.29x | 2.4 | 261.7 |
| rustique | 60 | 138.5 | 11.1 | 2,312.1 | 2,806.2 | 7.2 | 1.87x | 0.3 | 70.0 |
| eloquent | 60 | 155.6 | 83.6 | 498.0 | 2,257.5 | 6.4 | 2.10x | 2.4 | 247.7 |

### bulk_insert_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 15 | 1,219.2 | 109.6 | 2,942.2 | 2,942.2 | 0.8 | 1.00x | 3.2 | 70.1 |
| diesel | 15 | 1,220.9 | 135.6 | 2,894.9 | 2,894.9 | 0.8 | 1.00x | 1.5 | 566.6 |
| eloquent-opcache-jit | 15 | 7,211.0 | 6,938.6 | 9,381.9 | 9,381.9 | 0.1 | 5.91x | 22.8 | 302.8 |
| eloquent | 15 | 8,013.6 | 8,185.4 | 9,592.0 | 9,592.0 | 0.1 | 6.57x | 22.8 | 288.0 |

### eager_1000_users_10000_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 30 | 48.9 | 48.9 | 53.5 | 54.6 | 20.5 | 1.00x | 11.3 | 57.6 |
| diesel | 30 | 67.7 | 66.9 | 77.1 | 77.8 | 14.8 | 1.38x | 8.4 | 530.1 |
| eloquent-opcache-jit | 30 | 2,693.0 | 2,702.2 | 2,803.4 | 2,841.6 | 0.4 | 55.08x | 17.9 | 240.6 |
| eloquent | 30 | 2,705.4 | 2,674.2 | 2,867.2 | 2,941.8 | 0.4 | 55.33x | 17.9 | 230.3 |

### eager_50_users_500_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 600 | 3.1 | 3.1 | 3.6 | 4.5 | 317.6 | 1.00x | 0.5 | 43.5 |
| diesel | 600 | 3.6 | 3.5 | 5.0 | 6.2 | 274.5 | 1.16x | 0.4 | 382.6 |
| eloquent-opcache-jit | 600 | 136.3 | 133.3 | 150.8 | 209.3 | 7.3 | 43.28x | 1.1 | 239.2 |
| eloquent | 600 | 137.8 | 134.3 | 156.2 | 201.1 | 7.3 | 43.77x | 0.9 | 228.9 |

### fetch_100000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 15 | 285.2 | 284.9 | 301.0 | 301.0 | 3.5 | 1.00x | 51.2 | 94.1 |
| diesel | 15 | 325.2 | 307.4 | 436.9 | 436.9 | 3.1 | 1.14x | 78.4 | 382.4 |
| eloquent-opcache-jit | 15 | 909.6 | 897.3 | 1,006.5 | 1,006.5 | 1.1 | 3.19x | 148.2 | 238.6 |
| eloquent | 15 | 1,072.0 | 1,086.6 | 1,118.5 | 1,118.5 | 0.9 | 3.76x | 148.2 | 229.0 |

### fetch_10000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 60 | 29.8 | 29.7 | 32.7 | 33.9 | 33.6 | 1.00x | 5.8 | 45.4 |
| diesel | 60 | 34.8 | 32.4 | 60.9 | 61.8 | 28.7 | 1.17x | 8.0 | 71.9 |
| eloquent-opcache-jit | 60 | 83.8 | 83.5 | 99.6 | 109.2 | 11.9 | 2.82x | 14.9 | 103.5 |
| eloquent | 60 | 113.8 | 104.9 | 199.0 | 337.2 | 8.8 | 3.83x | 14.9 | 93.8 |

### fetch_1000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 300 | 3.2 | 3.0 | 4.3 | 4.7 | 314.7 | 1.00x | 0.8 | 41.2 |
| rustique | 300 | 3.5 | 3.3 | 4.0 | 14.2 | 286.8 | 1.10x | 0.5 | 39.5 |
| eloquent-opcache-jit | 300 | 8.3 | 8.0 | 10.1 | 13.2 | 120.0 | 2.62x | 1.5 | 89.5 |
| eloquent | 300 | 9.6 | 9.1 | 12.2 | 13.4 | 104.5 | 3.01x | 1.5 | 79.6 |

### find_by_pk

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 6000 | 152.6 | 146.0 | 195.9 | 252.9 | 6,553.6 | 1.00x | 0.1 | 37.6 |
| rustique | 6000 | 192.3 | 188.8 | 230.3 | 300.4 | 5,199.5 | 1.26x | 0.1 | 39.1 |
| eloquent-opcache-jit | 6000 | 230.4 | 217.3 | 319.4 | 458.3 | 4,340.5 | 1.51x | 0.4 | 88.5 |
| eloquent | 6000 | 231.2 | 216.1 | 331.3 | 388.2 | 4,325.3 | 1.52x | 0.0 | 78.4 |

### insert_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 601.1 | 569.9 | 856.0 | 971.2 | 1,663.6 | 1.00x | 0.0 | 70.0 |
| diesel | 3000 | 604.6 | 575.8 | 859.6 | 990.2 | 1,654.1 | 1.01x | 0.0 | 566.6 |
| eloquent-opcache-jit | 3000 | 651.5 | 619.8 | 923.7 | 1,097.1 | 1,534.9 | 1.08x | 0.1 | 258.6 |
| eloquent | 3000 | 800.5 | 702.9 | 1,138.6 | 1,665.8 | 1,249.2 | 1.33x | 0.0 | 244.0 |

### json_fetch_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 300 | 5.2 | 5.0 | 6.3 | 6.9 | 193.8 | 1.00x | 0.7 | 530.3 |
| rustique | 300 | 5.4 | 5.3 | 6.0 | 7.2 | 185.8 | 1.04x | 0.9 | 60.4 |
| eloquent | 300 | 265.6 | 260.7 | 292.1 | 337.1 | 3.8 | 51.46x | 2.1 | 230.9 |
| eloquent-opcache-jit | 300 | 273.1 | 265.8 | 331.5 | 367.2 | 3.7 | 52.93x | 2.1 | 241.1 |

### json_fetch_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 30 | 55.4 | 53.2 | 65.8 | 68.8 | 18.0 | 1.00x | 6.7 | 566.5 |
| rustique | 30 | 55.5 | 55.6 | 59.6 | 61.6 | 18.0 | 1.00x | 11.0 | 69.9 |
| eloquent-opcache-jit | 30 | 2,655.8 | 2,652.2 | 2,906.8 | 2,938.8 | 0.4 | 47.93x | 20.8 | 250.1 |
| eloquent | 30 | 2,859.9 | 2,673.7 | 3,668.4 | 4,020.1 | 0.3 | 51.62x | 20.8 | 240.0 |

### json_insert_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 608.6 | 581.8 | 862.0 | 974.8 | 1,643.1 | 1.00x | 0.0 | 70.0 |
| eloquent-opcache-jit | 3000 | 701.2 | 650.6 | 1,004.5 | 1,340.5 | 1,426.1 | 1.15x | 0.0 | 258.6 |
| eloquent | 3000 | 733.8 | 686.3 | 1,005.6 | 1,236.0 | 1,362.8 | 1.21x | 0.0 | 244.0 |
| diesel | 3000 | 1,226.0 | 600.8 | 895.2 | 1,072.1 | 815.7 | 2.01x | 0.0 | 566.6 |

### json_update_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 629.5 | 606.7 | 743.2 | 936.8 | 1,588.6 | 1.00x | 0.0 | 70.0 |
| diesel | 3000 | 636.3 | 611.5 | 772.9 | 930.8 | 1,571.5 | 1.01x | 0.0 | 566.6 |
| eloquent | 3000 | 994.1 | 784.8 | 1,458.0 | 1,822.5 | 1,006.0 | 1.58x | 1.1 | 244.0 |
| eloquent-opcache-jit | 3000 | 1,609.1 | 715.9 | 867.9 | 1,072.1 | 621.5 | 2.56x | 1.1 | 257.7 |

### json_where_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 513.5 | 507.0 | 588.9 | 681.5 | 1,947.4 | 1.00x | 0.0 | 69.9 |
| diesel | 3000 | 537.2 | 523.6 | 660.1 | 799.1 | 1,861.5 | 1.05x | 0.0 | 566.5 |
| eloquent-opcache-jit | 3000 | 6,004.7 | 5,827.3 | 7,164.2 | 8,911.9 | 166.5 | 11.69x | 0.2 | 250.6 |
| eloquent | 3000 | 6,054.9 | 5,900.9 | 7,125.0 | 8,223.3 | 165.2 | 11.79x | 0.0 | 240.0 |

### update_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 553.8 | 540.2 | 632.8 | 732.6 | 1,805.8 | 1.00x | 0.0 | 70.0 |
| diesel | 3000 | 564.1 | 546.2 | 650.6 | 824.0 | 1,772.8 | 1.02x | 0.0 | 566.6 |
| eloquent-opcache-jit | 3000 | 650.2 | 625.7 | 776.5 | 984.3 | 1,538.0 | 1.17x | 0.9 | 257.5 |
| eloquent | 3000 | 717.9 | 668.4 | 949.8 | 1,145.7 | 1,393.0 | 1.30x | 0.9 | 243.6 |

### where_order_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 557.1 | 565.2 | 681.5 | 797.9 | 1,794.9 | 1.00x | 0.0 | 39.2 |
| diesel | 3000 | 571.4 | 534.3 | 744.4 | 940.3 | 1,750.1 | 1.03x | 0.0 | 37.7 |
| eloquent | 3000 | 671.3 | 636.4 | 854.9 | 1,086.4 | 1,489.6 | 1.20x | 0.0 | 78.5 |
| eloquent-opcache-jit | 3000 | 700.5 | 615.0 | 889.3 | 1,962.6 | 1,427.5 | 1.26x | 0.1 | 88.6 |

