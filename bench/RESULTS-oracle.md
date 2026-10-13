# ORM benchmark results: oracle

3 rounds, timings pooled, memory as the median of the rounds. 2026-10-12, Linux 7.0.0-34-generic, rustc 1.96.1 (31fca3adb 2026-06-26), PHP 8.5.10, Oracle 21.3.0.0.0; diesel through diesel-oci, Eloquent through yajra/laravel-oci8.

## Summary: mean time per operation (fastest in bold)

| Scenario | diesel | eloquent-opcache-jit | eloquent | rustique |
|---|---:|---:|---:|---:|
| bulk_insert_1000 | 75.9 ms | 91.9 ms | 151.0 ms | **73.9 ms** |
| bulk_insert_10000 | 1,375.3 ms | 7,804.2 ms | 8,159.1 ms | **1,149.1 ms** |
| eager_1000_users_10000_posts | 74.5 ms | 2,798.9 ms | 2,778.0 ms | **55.4 ms** |
| eager_50_users_500_posts | **3.4 ms** | 138.2 ms | 139.1 ms | 3.5 ms |
| fetch_100000_rows | 334.6 ms | 969.4 ms | 1,067.9 ms | **292.6 ms** |
| fetch_10000_rows | 34.6 ms | 87.1 ms | 98.5 ms | **32.3 ms** |
| fetch_1000_rows | **3.2 ms** | 8.3 ms | 9.4 ms | 3.5 ms |
| find_by_pk | **150 µs** | 227 µs | 233 µs | 200 µs |
| insert_single | 629 µs | 1.3 ms | 700 µs | **622 µs** |
| json_fetch_1000 | **5.9 ms** | 282.9 ms | 275.0 ms | 6.3 ms |
| json_fetch_10000 | **59.4 ms** | 2,701.8 ms | 2,743.7 ms | 64.4 ms |
| json_insert_single | 642 µs | 692 µs | 726 µs | **641 µs** |
| json_update_single | **653 µs** | 761 µs | 857 µs | 675 µs |
| json_where_limit_20 | **572 µs** | 6.2 ms | 6.3 ms | 573 µs |
| update_single | **581 µs** | 657 µs | 675 µs | 587 µs |
| where_order_limit_20 | **552 µs** | 636 µs | 662 µs | 576 µs |

## Summary: memory

Heap peak: most memory in use above the start of the scenario, per allocator (Rust: global allocator; PHP: `memory_get_peak_usage()`). It misses what C client libraries take with `malloc`: libmysqlclient under diesel on MySQL, and Oracle's OCI under both Rust ORMs on Oracle. Peak RSS: whole process, highest during the scenario.

| Scenario | diesel heap MB | eloquent-opcache-jit heap MB | eloquent heap MB | rustique heap MB | diesel RSS MB | eloquent-opcache-jit RSS MB | eloquent RSS MB | rustique RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| bulk_insert_1000 | 0.1 | 2.4 | 2.4 | 0.3 | 566.6 | 261.6 | 247.5 | 86.2 |
| bulk_insert_10000 | 1.5 | 22.8 | 22.8 | 3.2 | 566.6 | 302.5 | 289.3 | 86.2 |
| eager_1000_users_10000_posts | 8.4 | 17.9 | 17.9 | 11.3 | 530.2 | 240.6 | 230.3 | 61.1 |
| eager_50_users_500_posts | 0.4 | 1.1 | 0.9 | 0.5 | 382.7 | 239.1 | 228.9 | 42.6 |
| fetch_100000_rows | 78.4 | 148.2 | 148.2 | 51.2 | 382.5 | 237.9 | 228.3 | 125.7 |
| fetch_10000_rows | 8.0 | 14.9 | 14.9 | 5.8 | 71.5 | 103.1 | 93.3 | 45.5 |
| fetch_1000_rows | 0.8 | 1.5 | 1.5 | 0.5 | 41.3 | 89.4 | 79.6 | 40.0 |
| find_by_pk | 0.1 | 0.4 | 0.0 | 0.1 | 37.7 | 88.4 | 78.3 | 39.1 |
| insert_single | 0.0 | 0.1 | 0.0 | 0.0 | 566.6 | 258.5 | 243.8 | 86.2 |
| json_fetch_1000 | 0.7 | 2.1 | 2.1 | 0.9 | 530.2 | 241.1 | 230.9 | 67.2 |
| json_fetch_10000 | 6.7 | 20.8 | 20.8 | 11.0 | 566.4 | 250.0 | 239.9 | 86.2 |
| json_insert_single | 0.0 | 0.0 | 0.0 | 0.0 | 566.6 | 258.5 | 243.8 | 86.2 |
| json_update_single | 0.0 | 1.1 | 1.1 | 0.0 | 566.6 | 257.6 | 243.8 | 86.2 |
| json_where_limit_20 | 0.0 | 0.2 | 0.0 | 0.0 | 566.4 | 250.5 | 240.0 | 86.2 |
| update_single | 0.0 | 0.9 | 0.9 | 0.0 | 566.4 | 257.5 | 243.5 | 86.2 |
| where_order_limit_20 | 0.0 | 0.1 | 0.0 | 0.0 | 37.8 | 88.4 | 78.4 | 39.6 |

## Per scenario

### bulk_insert_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 60 | 73.9 | 11.8 | 83.4 | 2,691.9 | 13.5 | 1.00x | 0.3 | 86.2 |
| diesel | 60 | 75.9 | 15.1 | 1,071.5 | 1,349.5 | 13.2 | 1.03x | 0.1 | 566.6 |
| eloquent-opcache-jit | 60 | 91.9 | 81.4 | 111.0 | 394.1 | 10.9 | 1.24x | 2.4 | 261.6 |
| eloquent | 60 | 151.0 | 85.2 | 151.7 | 2,261.7 | 6.6 | 2.04x | 2.4 | 247.5 |

### bulk_insert_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 15 | 1,149.1 | 123.7 | 2,917.1 | 2,917.1 | 0.9 | 1.00x | 3.2 | 86.2 |
| diesel | 15 | 1,375.3 | 209.9 | 2,877.8 | 2,877.8 | 0.7 | 1.20x | 1.5 | 566.6 |
| eloquent-opcache-jit | 15 | 7,804.2 | 7,457.5 | 9,540.5 | 9,540.5 | 0.1 | 6.79x | 22.8 | 302.5 |
| eloquent | 15 | 8,159.1 | 8,077.2 | 9,629.8 | 9,629.8 | 0.1 | 7.10x | 22.8 | 289.3 |

### eager_1000_users_10000_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 30 | 55.4 | 54.4 | 65.9 | 68.2 | 18.0 | 1.00x | 11.3 | 61.1 |
| diesel | 30 | 74.5 | 73.4 | 86.2 | 91.2 | 13.4 | 1.34x | 8.4 | 530.2 |
| eloquent | 30 | 2,778.0 | 2,771.4 | 2,933.0 | 3,005.1 | 0.4 | 50.11x | 17.9 | 230.3 |
| eloquent-opcache-jit | 30 | 2,798.9 | 2,771.8 | 2,966.9 | 3,018.2 | 0.4 | 50.49x | 17.9 | 240.6 |

### eager_50_users_500_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 600 | 3.4 | 3.4 | 4.0 | 4.3 | 293.3 | 1.00x | 0.4 | 382.7 |
| rustique | 600 | 3.5 | 3.4 | 4.6 | 5.4 | 282.5 | 1.04x | 0.5 | 42.6 |
| eloquent-opcache-jit | 600 | 138.2 | 137.0 | 146.8 | 157.1 | 7.2 | 40.54x | 1.1 | 239.1 |
| eloquent | 600 | 139.1 | 138.9 | 145.8 | 153.6 | 7.2 | 40.81x | 0.9 | 228.9 |

### fetch_100000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 15 | 292.6 | 290.2 | 314.8 | 314.8 | 3.4 | 1.00x | 51.2 | 125.7 |
| diesel | 15 | 334.6 | 333.7 | 348.1 | 348.1 | 3.0 | 1.14x | 78.4 | 382.5 |
| eloquent-opcache-jit | 15 | 969.4 | 959.7 | 1,196.8 | 1,196.8 | 1.0 | 3.31x | 148.2 | 237.9 |
| eloquent | 15 | 1,067.9 | 1,094.3 | 1,154.0 | 1,154.0 | 0.9 | 3.65x | 148.2 | 228.3 |

### fetch_10000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 60 | 32.3 | 31.1 | 37.0 | 58.7 | 31.0 | 1.00x | 5.8 | 45.5 |
| diesel | 60 | 34.6 | 34.1 | 38.1 | 47.7 | 28.9 | 1.07x | 8.0 | 71.5 |
| eloquent-opcache-jit | 60 | 87.1 | 87.2 | 96.6 | 98.6 | 11.5 | 2.70x | 14.9 | 103.1 |
| eloquent | 60 | 98.5 | 99.9 | 109.2 | 111.9 | 10.2 | 3.05x | 14.9 | 93.3 |

### fetch_1000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 300 | 3.2 | 3.2 | 3.7 | 4.2 | 311.3 | 1.00x | 0.8 | 41.3 |
| rustique | 300 | 3.5 | 3.4 | 4.2 | 4.9 | 286.8 | 1.09x | 0.5 | 40.0 |
| eloquent-opcache-jit | 300 | 8.3 | 8.1 | 10.1 | 12.2 | 120.0 | 2.59x | 1.5 | 89.4 |
| eloquent | 300 | 9.4 | 9.2 | 11.0 | 11.7 | 106.6 | 2.92x | 1.5 | 79.6 |

### find_by_pk

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 6000 | 149.9 | 143.7 | 198.3 | 248.1 | 6,671.8 | 1.00x | 0.1 | 37.7 |
| rustique | 6000 | 200.3 | 195.9 | 250.5 | 301.6 | 4,993.1 | 1.34x | 0.1 | 39.1 |
| eloquent-opcache-jit | 6000 | 226.9 | 219.7 | 290.9 | 422.7 | 4,406.5 | 1.51x | 0.4 | 88.4 |
| eloquent | 6000 | 232.9 | 223.2 | 321.8 | 404.9 | 4,294.0 | 1.55x | 0.0 | 78.3 |

### insert_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 621.5 | 588.9 | 885.7 | 1,037.7 | 1,609.0 | 1.00x | 0.0 | 86.2 |
| diesel | 3000 | 628.5 | 590.1 | 924.9 | 1,101.8 | 1,591.1 | 1.01x | 0.0 | 566.6 |
| eloquent | 3000 | 700.0 | 660.1 | 985.5 | 1,184.9 | 1,428.6 | 1.13x | 0.0 | 243.8 |
| eloquent-opcache-jit | 3000 | 1,288.0 | 628.1 | 959.3 | 1,130.3 | 776.4 | 2.07x | 0.1 | 258.5 |

### json_fetch_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 300 | 5.9 | 5.7 | 7.5 | 8.9 | 168.7 | 1.00x | 0.7 | 530.2 |
| rustique | 300 | 6.3 | 6.4 | 8.0 | 9.1 | 157.7 | 1.07x | 0.9 | 67.2 |
| eloquent | 300 | 275.0 | 274.4 | 288.5 | 298.9 | 3.6 | 46.38x | 2.1 | 230.9 |
| eloquent-opcache-jit | 300 | 282.9 | 281.1 | 312.7 | 321.0 | 3.5 | 47.71x | 2.1 | 241.1 |

### json_fetch_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 30 | 59.4 | 59.3 | 63.2 | 65.5 | 16.8 | 1.00x | 6.7 | 566.4 |
| rustique | 30 | 64.4 | 63.9 | 81.4 | 81.9 | 15.5 | 1.08x | 11.0 | 86.2 |
| eloquent-opcache-jit | 30 | 2,701.8 | 2,693.7 | 2,811.9 | 2,861.4 | 0.4 | 45.49x | 20.8 | 250.0 |
| eloquent | 30 | 2,743.7 | 2,747.8 | 2,840.0 | 2,842.3 | 0.4 | 46.20x | 20.8 | 239.9 |

### json_insert_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 3000 | 640.8 | 597.2 | 907.1 | 1,076.9 | 1,560.5 | 1.00x | 0.0 | 86.2 |
| diesel | 3000 | 642.5 | 610.3 | 928.5 | 1,079.3 | 1,556.5 | 1.00x | 0.0 | 566.6 |
| eloquent-opcache-jit | 3000 | 691.8 | 654.2 | 966.5 | 1,168.3 | 1,445.5 | 1.08x | 0.0 | 258.5 |
| eloquent | 3000 | 725.5 | 686.3 | 1,018.7 | 1,220.5 | 1,378.3 | 1.13x | 0.0 | 243.8 |

### json_update_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 3000 | 652.7 | 628.1 | 825.2 | 951.0 | 1,532.1 | 1.00x | 0.0 | 566.6 |
| rustique | 3000 | 674.9 | 642.3 | 866.7 | 1,089.9 | 1,481.7 | 1.03x | 0.0 | 86.2 |
| eloquent-opcache-jit | 3000 | 761.1 | 733.8 | 930.8 | 1,181.4 | 1,313.9 | 1.17x | 1.1 | 257.6 |
| eloquent | 3000 | 856.5 | 802.6 | 1,207.5 | 1,607.6 | 1,167.5 | 1.31x | 1.1 | 243.8 |

### json_where_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 3000 | 571.9 | 555.7 | 715.9 | 822.8 | 1,748.7 | 1.00x | 0.0 | 566.4 |
| rustique | 3000 | 573.0 | 543.8 | 748.0 | 974.8 | 1,745.1 | 1.00x | 0.0 | 86.2 |
| eloquent-opcache-jit | 3000 | 6,213.4 | 6,057.6 | 7,535.8 | 8,831.1 | 160.9 | 10.87x | 0.2 | 250.5 |
| eloquent | 3000 | 6,250.2 | 6,195.3 | 7,015.8 | 7,495.4 | 160.0 | 10.93x | 0.0 | 240.0 |

### update_single

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 3000 | 581.1 | 561.6 | 691.0 | 828.7 | 1,720.9 | 1.00x | 0.0 | 566.4 |
| rustique | 3000 | 586.9 | 564.0 | 714.8 | 915.4 | 1,703.9 | 1.01x | 0.0 | 86.2 |
| eloquent-opcache-jit | 3000 | 656.5 | 630.5 | 806.2 | 1,017.5 | 1,523.2 | 1.13x | 0.9 | 257.5 |
| eloquent | 3000 | 674.8 | 661.3 | 806.2 | 965.3 | 1,481.9 | 1.16x | 0.9 | 243.5 |

### where_order_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 3000 | 552.5 | 534.3 | 694.6 | 803.8 | 1,810.1 | 1.00x | 0.0 | 37.8 |
| rustique | 3000 | 576.4 | 575.8 | 748.0 | 882.2 | 1,734.8 | 1.04x | 0.0 | 39.6 |
| eloquent-opcache-jit | 3000 | 635.7 | 616.2 | 787.2 | 929.7 | 1,573.0 | 1.15x | 0.1 | 88.4 |
| eloquent | 3000 | 662.1 | 636.4 | 826.4 | 979.5 | 1,510.4 | 1.20x | 0.0 | 78.4 |

