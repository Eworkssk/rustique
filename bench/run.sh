#!/usr/bin/env bash
# Benchmarks rustique, diesel and Eloquent against the MySQL or Oracle from
# ../docker-compose.yml: `DB=oracle ./run.sh` (default mysql).
# Every runner starts from the same tables. Writes RESULTS-<db>.md.
set -euo pipefail
cd "$(dirname "$0")"

DB=${DB:-mysql}
ROUNDS=${ROUNDS:-5}
RUNNERS=(rustique diesel eloquent eloquent-opcache-jit)
RESULTS=results/$DB

MYSQL=(mysql -h127.0.0.1 -P13306 -uorm -psecret orm_test)
ORACLE=(docker compose -f ../docker-compose.yml exec -T oracle sqlplus -s -L orm/secret@//localhost:1521/XEPDB1)

# Runs SQL from stdin.
sql() {
    if [ "$DB" = oracle ]; then "${ORACLE[@]}" >/dev/null; else "${MYSQL[@]}" 2>/dev/null; fi
}

docker compose -f ../docker-compose.yml up -d "$DB"
if [ "$DB" = oracle ]; then
    until echo 'SELECT 1 FROM dual;' | "${ORACLE[@]}" 2>/dev/null | grep -q '^\s*1$'; do sleep 2; done
else
    until "${MYSQL[@]}" -e 'SELECT 1' >/dev/null 2>&1; do sleep 1; done
fi

(cd rust && cargo build --release --quiet)
[ -d php/vendor ] || (cd php && composer install --quiet)
mkdir -p "$RESULTS"
rm -f "$RESULTS"/*.json

# PHP without Xdebug (it slows every call and disables the JIT): same ini files minus that one.
PHP_SCAN=$(mktemp -d)
for ini in "$(php -r 'echo PHP_CONFIG_FILE_SCAN_DIR;')"/*.ini; do
    [[ $ini == *xdebug* ]] || ln -s "$ini" "$PHP_SCAN/"
done
export PHP_INI_SCAN_DIR=$PHP_SCAN

# MySQL: waits until InnoDB has flushed what earlier writes left behind; right
# after the seed it is busy for a while and queries run up to 50x slower.
settle() {
    [ "$DB" = mysql ] || return 0
    local dirty
    for _ in {1..300}; do
        dirty=$("${MYSQL[@]}" -N -e "SHOW GLOBAL STATUS LIKE 'Innodb_buffer_pool_pages_dirty'" 2>/dev/null | cut -f2)
        ((dirty < 100)) && return
        sleep 1
    done
    echo "warning: $dirty dirty pages left" >&2
}

echo "seeding $DB" >&2
if [ "$DB" = oracle ]; then sql < schema-oracle.sql; else sql < schema.sql; fi

run() {
    printf 'DELETE FROM bench_users WHERE id > 100000;\nDELETE FROM bench_events WHERE id > 100000;\nCOMMIT;\n' | sql
    settle
    case $1 in
        rustique | diesel) rust/target/release/orm-bench "$1" "$DB" "$2" ;;
        eloquent) php -d memory_limit=-1 php/bench.php "$DB" "$2" ;;
        eloquent-opcache-jit) php -d memory_limit=-1 -d opcache.enable_cli=1 -d opcache.jit=tracing -d opcache.jit_buffer_size=64M php/bench.php "$DB" "$2" ;;
    esac
}

# Rounds rotate the runner order so no ORM always runs first or last.
for ((round = 0; round < ROUNDS; round++)); do
    for ((k = 0; k < ${#RUNNERS[@]}; k++)); do
        name=${RUNNERS[(k + round) % ${#RUNNERS[@]}]}
        echo "round $((round + 1))/$ROUNDS: $name" >&2
        run "$name" "$RESULTS/$name.r$round.json"
    done
done

if [ "$DB" = oracle ]; then
    version="Oracle $(echo 'SELECT version_full FROM product_component_version;' | "${ORACLE[@]}" | grep -o '[0-9][0-9.]*' | head -1)"
    drivers="diesel through diesel-oci, Eloquent through yajra/laravel-oci8"
else
    version="MySQL $("${MYSQL[@]}" -N -e 'SELECT VERSION()' 2>/dev/null)"
    drivers="diesel with libmysqlclient, Eloquent with pdo_mysql"
fi
{
    echo "# ORM benchmark results: $DB"
    echo
    echo "$ROUNDS rounds, timings pooled, memory as the median of the rounds. $(date -u +%F), $(uname -sr), $(rustc --version), $(php -r 'echo "PHP ".PHP_VERSION;'), $version; $drivers."
    echo
    python3 report.py "$RESULTS"
} > "RESULTS-$DB.md"
[ -f RESULTS-mysql.md ] && [ -f RESULTS-oracle.md ] && python3 chart.py > ../assets/bench.svg
cat "RESULTS-$DB.md"
