-- Shared fixture, loaded once per run.sh: 100,000 users with 10 posts each and
-- 100,000 events with a JSON payload. run.sh deletes the rows the write
-- scenarios add (ids above these) before every runner.
DROP TABLE IF EXISTS bench_events;
DROP TABLE IF EXISTS bench_posts;
DROP TABLE IF EXISTS bench_users;

CREATE TABLE bench_users (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL,
    age INT NOT NULL,
    created_at DATETIME NULL,
    updated_at DATETIME NULL,
    KEY bench_users_age (age)
);

CREATE TABLE bench_posts (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    title VARCHAR(255) NOT NULL,
    body TEXT NOT NULL,
    created_at DATETIME NULL,
    updated_at DATETIME NULL,
    KEY bench_posts_user_id (user_id)
);

CREATE TABLE bench_events (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    payload JSON NOT NULL,
    created_at DATETIME NULL,
    updated_at DATETIME NULL
);

-- MySQL cannot join a temporary table to itself.
DROP TABLE IF EXISTS bench_seq;
CREATE TABLE bench_seq (n INT PRIMARY KEY);
INSERT INTO bench_seq
WITH RECURSIVE s (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 1000)
SELECT n FROM s;

INSERT INTO bench_users (name, email, age, created_at, updated_at)
SELECT CONCAT('User ', n), CONCAT('user', n, '@example.com'), 18 + n % 60, NOW(), NOW()
FROM (SELECT (a.n - 1) * 1000 + b.n AS n FROM bench_seq a JOIN bench_seq b WHERE a.n <= 100) t
ORDER BY n;

-- Post ids 1..10 belong to user 1, 11..20 to user 2, ...
INSERT INTO bench_posts (user_id, title, body, created_at, updated_at)
SELECT 1 + (n - 1) DIV 10, CONCAT('Post ', n), REPEAT('lorem ipsum ', 10), NOW(), NOW()
FROM (SELECT (a.n - 1) * 1000 + b.n AS n FROM bench_seq a JOIN bench_seq b) t
ORDER BY n;

INSERT INTO bench_events (user_id, payload, created_at, updated_at)
SELECT 1 + (n - 1) % 100000,
    JSON_OBJECT(
        'type', ELT(1 + n % 5, 'click', 'view', 'purchase', 'signup', 'logout'),
        'score', n,
        'tags', JSON_ARRAY('alpha', 'beta', 'gamma'),
        'meta', JSON_OBJECT('browser', 'firefox', 'os', 'linux', 'screen', JSON_OBJECT('w', 1920, 'h', 1080)),
        'items', JSON_ARRAY(JSON_OBJECT('sku', 'A1', 'qty', 2), JSON_OBJECT('sku', 'B2', 'qty', 1))
    ),
    NOW(), NOW()
FROM (SELECT (a.n - 1) * 1000 + b.n AS n FROM bench_seq a JOIN bench_seq b WHERE a.n <= 100) t
ORDER BY n;
DROP TABLE bench_seq;

-- Fresh statistics: right after the bulk load, the plan for an `IN` list of
-- 1,000 bound ids scans the whole posts table (1.7 s instead of 40 ms).
ANALYZE TABLE bench_users, bench_posts, bench_events;
