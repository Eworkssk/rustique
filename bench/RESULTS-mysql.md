# ORM benchmark results: mysql

5 rounds, timings pooled, memory as the median of the rounds. 2026-10-13, Linux 7.0.0-34-generic, rustc 1.96.1 (31fca3adb 2026-06-26), PHP 8.5.10, MySQL 8.4.11; diesel with libmysqlclient, Eloquent with pdo_mysql.

## Summary: mean time per operation (fastest in bold)

| Scenario | diesel | eloquent-opcache-jit | eloquent | rustique |
|---|---:|---:|---:|---:|
| bulk_insert_1000 | 10.7 ms | 12.3 ms | 11.0 ms | **6.8 ms** |
| bulk_insert_10000 | 121.6 ms | 152.2 ms | 136.3 ms | **86.0 ms** |
| eager_1000_users_10000_posts | **15.7 ms** | 81.8 ms | 99.1 ms | 31.5 ms |
| eager_50_users_500_posts | **1.3 ms** | 4.4 ms | 5.4 ms | 2.2 ms |
| fetch_100000_rows | **55.3 ms** | 507.8 ms | 594.9 ms | 215.5 ms |
| fetch_10000_rows | **6.1 ms** | 44.7 ms | 53.9 ms | 21.3 ms |
| fetch_1000_rows | **660 µs** | 4.6 ms | 5.3 ms | 2.5 ms |
| find_by_pk | **102 µs** | 279 µs | 265 µs | 136 µs |
| insert_single | 1.2 ms | 1.3 ms | 1.3 ms | **1.1 ms** |
| json_fetch_1000 | **2.2 ms** | 6.3 ms | 6.9 ms | 3.7 ms |
| json_fetch_10000 | **22.4 ms** | 64.3 ms | 73.3 ms | 36.1 ms |
| json_insert_single | 1.2 ms | 1.3 ms | 1.3 ms | **1.1 ms** |
| json_update_single | 1.3 ms | 1.4 ms | 1.4 ms | **1.2 ms** |
| json_where_limit_20 | 306 µs | 481 µs | 439 µs | **257 µs** |
| update_single | 1.2 ms | 1.4 ms | 1.3 ms | **1.1 ms** |
| where_order_limit_20 | **175 µs** | 496 µs | 440 µs | 337 µs |

## Summary: memory

Heap peak: most memory in use above the start of the scenario, per allocator (Rust: global allocator; PHP: `memory_get_peak_usage()`). It misses what C client libraries take with `malloc`: libmysqlclient under diesel on MySQL, and Oracle's OCI under both Rust ORMs on Oracle. Peak RSS: whole process, highest during the scenario.

| Scenario | diesel heap MB | eloquent-opcache-jit heap MB | eloquent heap MB | rustique heap MB | diesel RSS MB | eloquent-opcache-jit RSS MB | eloquent RSS MB | rustique RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| bulk_insert_1000 | 1.0 | 1.8 | 1.8 | 2.0 | 46.6 | 214.6 | 200.7 | 65.5 |
| bulk_insert_10000 | 10.0 | 17.6 | 17.6 | 15.5 | 54.8 | 222.0 | 209.0 | 65.5 |
| eager_1000_users_10000_posts | 4.1 | 15.1 | 15.1 | 11.4 | 46.2 | 199.2 | 189.5 | 65.4 |
| eager_50_users_500_posts | 0.2 | 1.0 | 0.8 | 0.5 | 46.2 | 195.1 | 185.4 | 65.4 |
| fetch_100000_rows | 14.0 | 123.0 | 123.0 | 51.2 | 46.2 | 202.9 | 193.5 | 65.4 |
| fetch_10000_rows | 1.7 | 12.4 | 12.4 | 5.8 | 17.2 | 85.5 | 76.4 | 15.6 |
| fetch_1000_rows | 0.1 | 1.2 | 1.2 | 0.5 | 14.2 | 72.9 | 63.6 | 10.8 |
| find_by_pk | 0.1 | 0.4 | 0.0 | 0.1 | 13.8 | 72.2 | 62.2 | 10.4 |
| insert_single | 0.0 | 0.1 | 0.0 | 0.0 | 46.6 | 214.1 | 200.2 | 65.5 |
| json_fetch_1000 | 0.4 | 1.9 | 1.9 | 0.9 | 46.5 | 199.2 | 189.5 | 65.5 |
| json_fetch_10000 | 5.4 | 18.7 | 18.7 | 11.0 | 46.5 | 208.4 | 198.3 | 65.5 |
| json_insert_single | 0.0 | 0.0 | 0.0 | 0.0 | 46.6 | 214.1 | 200.2 | 65.5 |
| json_update_single | 0.0 | 1.1 | 1.1 | 0.0 | 46.6 | 214.0 | 200.2 | 65.5 |
| json_where_limit_20 | 0.0 | 0.2 | 0.1 | 0.0 | 46.5 | 208.8 | 198.4 | 65.5 |
| update_single | 0.0 | 0.9 | 0.9 | 0.0 | 46.5 | 214.0 | 200.1 | 65.5 |
| where_order_limit_20 | 0.0 | 0.1 | 0.1 | 0.0 | 13.8 | 72.2 | 62.2 | 10.4 |

## Per scenario

### bulk_insert_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 100 | 6.8 | 6.7 | 8.4 | 10.1 | 147.0 | 1.00x | 2.0 | 65.5 |
| diesel | 100 | 10.7 | 10.7 | 12.3 | 13.0 | 93.1 | 1.58x | 1.0 | 46.6 |
| eloquent | 100 | 11.0 | 10.9 | 13.8 | 14.4 | 90.9 | 1.62x | 1.8 | 200.7 |
| eloquent-opcache-jit | 100 | 12.3 | 12.0 | 16.4 | 39.8 | 81.6 | 1.80x | 1.8 | 214.6 |

### bulk_insert_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 25 | 86.0 | 62.8 | 182.0 | 193.1 | 11.6 | 1.00x | 15.5 | 65.5 |
| diesel | 25 | 121.6 | 121.5 | 131.9 | 135.0 | 8.2 | 1.41x | 10.0 | 54.8 |
| eloquent | 25 | 136.3 | 133.3 | 156.4 | 171.8 | 7.3 | 1.58x | 17.6 | 209.0 |
| eloquent-opcache-jit | 25 | 152.2 | 150.4 | 203.5 | 228.2 | 6.6 | 1.77x | 17.6 | 222.0 |

### eager_1000_users_10000_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 50 | 15.7 | 15.6 | 17.3 | 18.7 | 63.7 | 1.00x | 4.1 | 46.2 |
| rustique | 50 | 31.5 | 30.5 | 36.8 | 37.8 | 31.8 | 2.00x | 11.4 | 65.4 |
| eloquent-opcache-jit | 50 | 81.8 | 82.3 | 91.3 | 99.7 | 12.2 | 5.21x | 15.1 | 199.2 |
| eloquent | 50 | 99.1 | 99.0 | 117.6 | 142.4 | 10.1 | 6.31x | 15.1 | 189.5 |

### eager_50_users_500_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 1000 | 1.3 | 1.3 | 1.7 | 2.2 | 741.3 | 1.00x | 0.2 | 46.2 |
| rustique | 1000 | 2.2 | 2.0 | 2.9 | 3.7 | 462.6 | 1.60x | 0.5 | 65.4 |
| eloquent-opcache-jit | 1000 | 4.4 | 4.1 | 5.6 | 7.6 | 228.8 | 3.24x | 1.0 | 195.1 |
| eloquent | 1000 | 5.4 | 4.9 | 7.9 | 9.3 | 184.1 | 4.03x | 0.8 | 185.4 |

### fetch_100000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 25 | 55.3 | 55.0 | 60.4 | 64.3 | 18.1 | 1.00x | 14.0 | 46.2 |
| rustique | 25 | 215.5 | 210.2 | 248.0 | 265.1 | 4.6 | 3.90x | 51.2 | 65.4 |
| eloquent-opcache-jit | 25 | 507.8 | 515.6 | 560.9 | 602.4 | 2.0 | 9.19x | 123.0 | 202.9 |
| eloquent | 25 | 594.9 | 596.2 | 674.9 | 699.7 | 1.7 | 10.76x | 123.0 | 193.5 |

### fetch_10000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 100 | 6.1 | 5.7 | 6.9 | 26.7 | 163.5 | 1.00x | 1.7 | 17.2 |
| rustique | 100 | 21.3 | 21.2 | 22.8 | 25.2 | 47.0 | 3.48x | 5.8 | 15.6 |
| eloquent-opcache-jit | 100 | 44.7 | 44.6 | 53.1 | 76.0 | 22.4 | 7.30x | 12.4 | 85.5 |
| eloquent | 100 | 53.9 | 53.4 | 67.6 | 94.4 | 18.6 | 8.81x | 12.4 | 76.4 |

### fetch_1000_rows

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 500 | 659.8 | 628.1 | 825.2 | 998.5 | 1,515.5 | 1.00x | 0.1 | 14.2 |
| rustique | 500 | 2,493.1 | 2,320.0 | 2,892.2 | 4,332.5 | 401.1 | 3.78x | 0.5 | 10.8 |
| eloquent-opcache-jit | 500 | 4,577.9 | 4,181.7 | 6,088.5 | 7,845.7 | 218.4 | 6.94x | 1.2 | 72.9 |
| eloquent | 500 | 5,342.0 | 5,241.9 | 6,331.9 | 6,990.8 | 187.2 | 8.10x | 1.2 | 63.6 |

### find_by_pk

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 10000 | 102.0 | 99.7 | 128.2 | 171.0 | 9,800.6 | 1.00x | 0.1 | 13.8 |
| rustique | 10000 | 135.5 | 125.9 | 187.6 | 237.5 | 7,379.4 | 1.33x | 0.1 | 10.4 |
| eloquent | 10000 | 265.0 | 249.3 | 360.9 | 463.0 | 3,773.3 | 2.60x | 0.0 | 62.2 |
| eloquent-opcache-jit | 10000 | 278.7 | 237.5 | 396.6 | 609.1 | 3,588.5 | 2.73x | 0.4 | 72.2 |

### insert_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 5000 | 1.1 | 1.0 | 1.3 | 1.5 | 941.1 | 1.00x | 0.0 | 65.5 |
| diesel | 5000 | 1.2 | 1.2 | 1.4 | 1.7 | 822.0 | 1.15x | 0.0 | 46.6 |
| eloquent | 5000 | 1.3 | 1.3 | 1.5 | 1.8 | 775.5 | 1.21x | 0.0 | 200.2 |
| eloquent-opcache-jit | 5000 | 1.3 | 1.3 | 1.5 | 1.9 | 768.5 | 1.22x | 0.1 | 214.1 |

### json_fetch_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 500 | 2.2 | 2.1 | 2.6 | 2.9 | 463.1 | 1.00x | 0.4 | 46.5 |
| rustique | 500 | 3.7 | 3.6 | 4.6 | 5.6 | 268.0 | 1.73x | 0.9 | 65.5 |
| eloquent-opcache-jit | 500 | 6.3 | 6.4 | 8.1 | 8.7 | 157.7 | 2.94x | 1.9 | 199.2 |
| eloquent | 500 | 6.9 | 6.7 | 8.4 | 9.5 | 145.7 | 3.18x | 1.9 | 189.5 |

### json_fetch_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 50 | 22.4 | 21.0 | 30.5 | 50.4 | 44.7 | 1.00x | 5.4 | 46.5 |
| rustique | 50 | 36.1 | 35.5 | 42.3 | 45.7 | 27.7 | 1.61x | 11.0 | 65.5 |
| eloquent-opcache-jit | 50 | 64.3 | 65.7 | 72.3 | 77.8 | 15.6 | 2.87x | 18.7 | 208.4 |
| eloquent | 50 | 73.3 | 74.8 | 85.4 | 91.8 | 13.6 | 3.28x | 18.7 | 198.3 |

### json_insert_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 5000 | 1.1 | 1.0 | 1.3 | 1.6 | 912.4 | 1.00x | 0.0 | 65.5 |
| diesel | 5000 | 1.2 | 1.2 | 1.4 | 1.7 | 818.9 | 1.11x | 0.0 | 46.6 |
| eloquent | 5000 | 1.3 | 1.3 | 1.4 | 1.7 | 775.1 | 1.18x | 0.0 | 200.2 |
| eloquent-opcache-jit | 5000 | 1.3 | 1.3 | 1.5 | 1.9 | 764.2 | 1.19x | 0.0 | 214.1 |

### json_update_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 5000 | 1.2 | 1.1 | 1.4 | 1.9 | 846.9 | 1.00x | 0.0 | 65.5 |
| diesel | 5000 | 1.3 | 1.2 | 1.4 | 1.8 | 796.6 | 1.06x | 0.0 | 46.6 |
| eloquent | 5000 | 1.4 | 1.3 | 1.6 | 2.0 | 726.3 | 1.17x | 1.1 | 200.2 |
| eloquent-opcache-jit | 5000 | 1.4 | 1.4 | 1.8 | 2.1 | 706.9 | 1.20x | 1.1 | 214.0 |

### json_where_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 5000 | 257.3 | 245.8 | 307.5 | 360.9 | 3,886.9 | 1.00x | 0.0 | 65.5 |
| diesel | 5000 | 306.5 | 293.3 | 387.1 | 497.5 | 3,263.0 | 1.19x | 0.0 | 46.5 |
| eloquent | 5000 | 439.4 | 413.2 | 572.3 | 738.5 | 2,275.9 | 1.71x | 0.1 | 198.4 |
| eloquent-opcache-jit | 5000 | 480.6 | 432.2 | 641.1 | 915.4 | 2,080.7 | 1.87x | 0.2 | 208.8 |

### update_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 5000 | 1.1 | 1.1 | 1.3 | 1.6 | 902.4 | 1.00x | 0.0 | 65.5 |
| diesel | 5000 | 1.2 | 1.2 | 1.4 | 1.8 | 808.9 | 1.12x | 0.0 | 46.5 |
| eloquent | 5000 | 1.3 | 1.3 | 1.5 | 2.0 | 749.0 | 1.20x | 0.9 | 200.1 |
| eloquent-opcache-jit | 5000 | 1.4 | 1.3 | 1.7 | 2.1 | 723.0 | 1.25x | 0.9 | 214.0 |

### where_order_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 5000 | 175.0 | 161.5 | 244.6 | 326.5 | 5,715.3 | 1.00x | 0.0 | 13.8 |
| rustique | 5000 | 337.2 | 225.6 | 607.9 | 927.3 | 2,965.3 | 1.93x | 0.0 | 10.4 |
| eloquent | 5000 | 440.3 | 423.9 | 568.7 | 710.0 | 2,271.2 | 2.52x | 0.1 | 62.2 |
| eloquent-opcache-jit | 5000 | 495.9 | 390.6 | 774.1 | 1,448.5 | 2,016.6 | 2.83x | 0.1 | 72.2 |

