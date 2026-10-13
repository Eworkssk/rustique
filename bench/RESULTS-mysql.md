# ORM benchmark results: mysql

5 rounds, timings pooled, memory as the median of the rounds. 2026-10-12, Linux 7.0.0-34-generic, rustc 1.96.1 (31fca3adb 2026-06-26), PHP 8.5.10, MySQL 8.4.11; diesel with libmysqlclient, Eloquent with pdo_mysql.

## Summary: mean time per operation (fastest in bold)

| Scenario | diesel | eloquent-opcache-jit | eloquent | rustique |
|---|---:|---:|---:|---:|
| bulk_insert_1000 | 12.8 ms | 12.7 ms | 13.3 ms | **9.0 ms** |
| bulk_insert_10000 | 134.4 ms | 152.5 ms | 163.2 ms | **94.0 ms** |
| eager_1000_users_10000_posts | **18.0 ms** | 89.6 ms | 102.4 ms | 32.0 ms |
| eager_50_users_500_posts | **1.4 ms** | 4.6 ms | 5.6 ms | 2.2 ms |
| fetch_100000_rows | **61.7 ms** | 518.9 ms | 615.5 ms | 217.7 ms |
| fetch_10000_rows | **6.8 ms** | 45.9 ms | 56.9 ms | 21.4 ms |
| fetch_1000_rows | **737 µs** | 4.6 ms | 5.6 ms | 2.4 ms |
| find_by_pk | **109 µs** | 268 µs | 275 µs | 135 µs |
| insert_single | 2.1 ms | **1.3 ms** | 1.6 ms | 1.9 ms |
| json_fetch_1000 | **2.5 ms** | 6.4 ms | 7.5 ms | 3.8 ms |
| json_fetch_10000 | **23.7 ms** | 68.9 ms | 78.6 ms | 37.6 ms |
| json_insert_single | 1.9 ms | **1.3 ms** | 1.4 ms | 2.1 ms |
| json_update_single | 2.1 ms | **1.4 ms** | 2.1 ms | 1.8 ms |
| json_where_limit_20 | 344 µs | 446 µs | 468 µs | **279 µs** |
| update_single | 2.1 ms | **1.3 ms** | 2.0 ms | 1.8 ms |
| where_order_limit_20 | **191 µs** | 424 µs | 478 µs | 294 µs |

## Summary: memory

Heap peak: most memory in use above the start of the scenario, per allocator (Rust: global allocator; PHP: `memory_get_peak_usage()`). It misses what C client libraries take with `malloc`: libmysqlclient under diesel on MySQL, and Oracle's OCI under both Rust ORMs on Oracle. Peak RSS: whole process, highest during the scenario.

| Scenario | diesel heap MB | eloquent-opcache-jit heap MB | eloquent heap MB | rustique heap MB | diesel RSS MB | eloquent-opcache-jit RSS MB | eloquent RSS MB | rustique RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| bulk_insert_1000 | 1.0 | 1.8 | 1.8 | 2.0 | 46.0 | 214.0 | 200.2 | 64.9 |
| bulk_insert_10000 | 10.0 | 17.6 | 17.6 | 15.5 | 54.2 | 220.9 | 208.3 | 64.9 |
| eager_1000_users_10000_posts | 4.1 | 15.1 | 15.1 | 11.4 | 46.0 | 198.5 | 189.0 | 64.8 |
| eager_50_users_500_posts | 0.2 | 1.0 | 0.8 | 0.5 | 46.0 | 194.4 | 184.9 | 64.8 |
| fetch_100000_rows | 14.0 | 123.0 | 123.0 | 51.2 | 45.9 | 202.0 | 192.8 | 64.8 |
| fetch_10000_rows | 1.7 | 12.4 | 12.4 | 5.8 | 16.9 | 84.8 | 75.8 | 14.9 |
| fetch_1000_rows | 0.1 | 1.2 | 1.2 | 0.5 | 13.9 | 72.5 | 63.4 | 10.2 |
| find_by_pk | 0.1 | 0.4 | 0.0 | 0.1 | 13.5 | 71.8 | 61.9 | 9.8 |
| insert_single | 0.0 | 0.1 | 0.0 | 0.0 | 46.0 | 213.5 | 199.7 | 64.9 |
| json_fetch_1000 | 0.4 | 1.9 | 1.9 | 0.9 | 46.0 | 198.5 | 189.0 | 64.9 |
| json_fetch_10000 | 5.4 | 18.7 | 18.7 | 11.0 | 46.0 | 207.7 | 197.9 | 64.9 |
| json_insert_single | 0.0 | 0.0 | 0.0 | 0.0 | 46.0 | 213.5 | 199.7 | 64.9 |
| json_update_single | 0.0 | 1.1 | 1.1 | 0.0 | 46.0 | 213.4 | 199.7 | 64.9 |
| json_where_limit_20 | 0.0 | 0.2 | 0.1 | 0.0 | 46.0 | 208.1 | 197.9 | 64.9 |
| update_single | 0.0 | 0.9 | 0.9 | 0.0 | 46.0 | 213.3 | 199.6 | 64.9 |
| where_order_limit_20 | 0.0 | 0.1 | 0.1 | 0.0 | 13.5 | 71.8 | 61.9 | 9.8 |

## Per scenario

### bulk_insert_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 100 | 9.0 | 7.7 | 15.1 | 24.9 | 110.9 | 1.00x | 2.0 | 64.9 |
| eloquent-opcache-jit | 100 | 12.7 | 12.4 | 15.2 | 30.2 | 78.8 | 1.41x | 1.8 | 214.0 |
| diesel | 100 | 12.8 | 12.4 | 16.0 | 21.2 | 78.2 | 1.42x | 1.0 | 46.0 |
| eloquent | 100 | 13.3 | 13.2 | 16.8 | 17.2 | 75.2 | 1.48x | 1.8 | 200.2 |

### bulk_insert_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 25 | 94.0 | 71.6 | 182.0 | 200.9 | 10.6 | 1.00x | 15.5 | 64.9 |
| diesel | 25 | 134.4 | 129.8 | 161.3 | 168.3 | 7.4 | 1.43x | 10.0 | 54.2 |
| eloquent-opcache-jit | 25 | 152.5 | 152.1 | 165.9 | 166.4 | 6.6 | 1.62x | 17.6 | 220.9 |
| eloquent | 25 | 163.2 | 163.2 | 173.9 | 177.2 | 6.1 | 1.74x | 17.6 | 208.3 |

### eager_1000_users_10000_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 50 | 18.0 | 17.2 | 22.1 | 32.7 | 55.4 | 1.00x | 4.1 | 46.0 |
| rustique | 50 | 32.0 | 31.1 | 37.3 | 38.3 | 31.2 | 1.77x | 11.4 | 64.8 |
| eloquent-opcache-jit | 50 | 89.6 | 89.5 | 100.6 | 123.0 | 11.2 | 4.97x | 15.1 | 198.5 |
| eloquent | 50 | 102.4 | 102.7 | 112.3 | 115.2 | 9.8 | 5.68x | 15.1 | 189.0 |

### eager_50_users_500_posts

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 1000 | 1.4 | 1.3 | 2.2 | 2.8 | 692.9 | 1.00x | 0.2 | 46.0 |
| rustique | 1000 | 2.2 | 2.0 | 3.1 | 3.3 | 463.6 | 1.49x | 0.5 | 64.8 |
| eloquent-opcache-jit | 1000 | 4.6 | 4.5 | 5.5 | 6.0 | 219.5 | 3.16x | 1.0 | 194.4 |
| eloquent | 1000 | 5.6 | 5.4 | 7.2 | 8.3 | 178.1 | 3.89x | 0.8 | 184.9 |

### fetch_100000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 25 | 61.7 | 60.2 | 67.4 | 95.4 | 16.2 | 1.00x | 14.0 | 45.9 |
| rustique | 25 | 217.7 | 215.5 | 234.0 | 236.6 | 4.6 | 3.53x | 51.2 | 64.8 |
| eloquent-opcache-jit | 25 | 518.9 | 527.2 | 568.0 | 596.4 | 1.9 | 8.40x | 123.0 | 202.0 |
| eloquent | 25 | 615.5 | 626.0 | 710.7 | 744.1 | 1.6 | 9.97x | 123.0 | 192.8 |

### fetch_10000_rows

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 100 | 6.8 | 6.1 | 8.6 | 30.6 | 147.7 | 1.00x | 1.7 | 16.9 |
| rustique | 100 | 21.4 | 21.3 | 22.7 | 24.3 | 46.8 | 3.16x | 5.8 | 14.9 |
| eloquent-opcache-jit | 100 | 45.9 | 45.4 | 53.4 | 55.6 | 21.8 | 6.77x | 12.4 | 84.8 |
| eloquent | 100 | 56.9 | 56.0 | 68.2 | 75.2 | 17.6 | 8.40x | 12.4 | 75.8 |

### fetch_1000_rows

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 500 | 736.8 | 705.3 | 924.9 | 1,028.2 | 1,357.2 | 1.00x | 0.1 | 13.9 |
| rustique | 500 | 2,380.6 | 2,354.4 | 2,685.7 | 2,837.7 | 420.1 | 3.23x | 0.5 | 10.2 |
| eloquent-opcache-jit | 500 | 4,582.6 | 4,448.8 | 5,485.3 | 6,127.7 | 218.2 | 6.22x | 1.2 | 72.5 |
| eloquent | 500 | 5,596.4 | 5,454.5 | 6,660.8 | 8,017.9 | 178.7 | 7.60x | 1.2 | 63.4 |

### find_by_pk

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 10000 | 108.6 | 103.3 | 144.9 | 192.3 | 9,205.8 | 1.00x | 0.1 | 13.5 |
| rustique | 10000 | 135.2 | 129.4 | 179.3 | 224.4 | 7,395.0 | 1.24x | 0.1 | 9.8 |
| eloquent-opcache-jit | 10000 | 268.3 | 254.1 | 364.5 | 531.9 | 3,727.7 | 2.47x | 0.4 | 71.8 |
| eloquent | 10000 | 275.4 | 258.8 | 379.9 | 496.3 | 3,631.7 | 2.53x | 0.0 | 61.9 |

### insert_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| eloquent-opcache-jit | 5000 | 1.3 | 1.3 | 1.6 | 1.9 | 751.6 | 1.00x | 0.1 | 213.5 |
| eloquent | 5000 | 1.6 | 1.3 | 4.1 | 6.0 | 617.9 | 1.22x | 0.0 | 199.7 |
| rustique | 5000 | 1.9 | 1.1 | 5.5 | 7.1 | 517.6 | 1.45x | 0.0 | 64.9 |
| diesel | 5000 | 2.1 | 1.3 | 5.7 | 7.1 | 468.7 | 1.60x | 0.0 | 46.0 |

### json_fetch_1000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 500 | 2.5 | 2.4 | 3.7 | 4.1 | 392.2 | 1.00x | 0.4 | 46.0 |
| rustique | 500 | 3.8 | 3.8 | 4.4 | 4.7 | 261.6 | 1.50x | 0.9 | 64.9 |
| eloquent-opcache-jit | 500 | 6.4 | 6.1 | 8.0 | 10.1 | 156.3 | 2.51x | 1.9 | 198.5 |
| eloquent | 500 | 7.5 | 7.4 | 8.7 | 9.5 | 132.6 | 2.96x | 1.9 | 189.0 |

### json_fetch_10000

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 50 | 23.7 | 23.0 | 31.0 | 35.2 | 42.2 | 1.00x | 5.4 | 46.0 |
| rustique | 50 | 37.6 | 37.6 | 40.8 | 41.2 | 26.6 | 1.59x | 11.0 | 64.9 |
| eloquent-opcache-jit | 50 | 68.9 | 68.7 | 77.4 | 77.8 | 14.5 | 2.91x | 18.7 | 207.7 |
| eloquent | 50 | 78.6 | 78.9 | 83.7 | 84.3 | 12.7 | 3.32x | 18.7 | 197.9 |

### json_insert_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| eloquent-opcache-jit | 5000 | 1.3 | 1.3 | 1.5 | 1.7 | 760.3 | 1.00x | 0.0 | 213.5 |
| eloquent | 5000 | 1.4 | 1.3 | 1.8 | 2.1 | 723.0 | 1.05x | 0.0 | 199.7 |
| diesel | 5000 | 1.9 | 1.3 | 5.3 | 6.9 | 522.4 | 1.46x | 0.0 | 46.0 |
| rustique | 5000 | 2.1 | 1.1 | 6.6 | 8.0 | 480.8 | 1.58x | 0.0 | 64.9 |

### json_update_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| eloquent-opcache-jit | 5000 | 1.4 | 1.4 | 1.8 | 2.1 | 702.7 | 1.00x | 1.1 | 213.4 |
| rustique | 5000 | 1.8 | 1.2 | 5.3 | 6.8 | 551.0 | 1.28x | 0.0 | 64.9 |
| eloquent | 5000 | 2.1 | 1.4 | 5.5 | 6.8 | 487.3 | 1.44x | 1.1 | 199.7 |
| diesel | 5000 | 2.1 | 1.3 | 5.3 | 6.9 | 471.3 | 1.49x | 0.0 | 46.0 |

### json_where_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rustique | 5000 | 278.9 | 269.5 | 358.6 | 416.7 | 3,585.9 | 1.00x | 0.0 | 64.9 |
| diesel | 5000 | 343.7 | 314.6 | 507.0 | 628.1 | 2,909.6 | 1.23x | 0.0 | 46.0 |
| eloquent-opcache-jit | 5000 | 445.9 | 425.1 | 598.4 | 737.3 | 2,242.9 | 1.60x | 0.2 | 208.1 |
| eloquent | 5000 | 468.3 | 447.6 | 604.3 | 733.8 | 2,135.2 | 1.68x | 0.1 | 197.9 |

### update_single

| ORM | runs | mean ms | p50 ms | p95 ms | p99 ms | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| eloquent-opcache-jit | 5000 | 1.3 | 1.3 | 1.6 | 1.8 | 741.4 | 1.00x | 0.9 | 213.3 |
| rustique | 5000 | 1.8 | 1.1 | 5.3 | 6.8 | 566.7 | 1.31x | 0.0 | 64.9 |
| eloquent | 5000 | 2.0 | 1.4 | 5.3 | 6.8 | 504.9 | 1.47x | 0.9 | 199.6 |
| diesel | 5000 | 2.1 | 1.3 | 5.2 | 6.8 | 479.2 | 1.55x | 0.0 | 46.0 |

### where_order_limit_20

| ORM | runs | mean µs | p50 µs | p95 µs | p99 µs | ops/s | vs fastest | heap peak MB | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| diesel | 5000 | 190.8 | 176.9 | 267.1 | 346.7 | 5,241.5 | 1.00x | 0.0 | 13.5 |
| rustique | 5000 | 293.8 | 243.4 | 520.0 | 591.3 | 3,403.9 | 1.54x | 0.0 | 9.8 |
| eloquent-opcache-jit | 5000 | 423.8 | 403.7 | 566.3 | 734.9 | 2,359.5 | 2.22x | 0.1 | 71.8 |
| eloquent | 5000 | 477.9 | 451.2 | 666.1 | 908.3 | 2,092.6 | 2.50x | 0.1 | 61.9 |

