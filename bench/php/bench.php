<?php

// Runs the shared ORM scenarios with Eloquent and writes, per scenario, the
// per-operation timings (nanoseconds) and memory figures as JSON:
// `php bench.php <mysql|oracle> <out.json>`.

require __DIR__.'/vendor/autoload.php';

use Illuminate\Database\Capsule\Manager as Capsule;
use Illuminate\Database\Connection;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\HasMany;
use Yajra\Oci8\Connectors\OracleConnector;
use Yajra\Oci8\Oci8Connection;

[, $database, $outPath] = $argv;

$capsule = new Capsule;
if ($database === 'oracle') {
    // What yajra's Oci8ServiceProvider registers in a Laravel app.
    $capsule->getContainer()->bind('db.connector.oracle', OracleConnector::class);
    Connection::resolverFor('oracle', function ($pdo, $database, $prefix, $config) {
        $db = new Oci8Connection((new OracleConnector)->connect($config), $database, $prefix, $config);
        $db->setSessionVars([
            'NLS_TIME_FORMAT' => 'HH24:MI:SS',
            'NLS_DATE_FORMAT' => 'YYYY-MM-DD HH24:MI:SS',
            'NLS_TIMESTAMP_FORMAT' => 'YYYY-MM-DD HH24:MI:SS',
            'NLS_TIMESTAMP_TZ_FORMAT' => 'YYYY-MM-DD HH24:MI:SS TZH:TZM',
            'NLS_NUMERIC_CHARACTERS' => '.,',
        ]);

        return $db;
    });
    $capsule->addConnection([
        'driver' => 'oracle',
        'host' => '127.0.0.1',
        'port' => 11521,
        'service_name' => 'XEPDB1',
        'database' => '',
        'username' => 'orm',
        'password' => 'secret',
        'charset' => 'AL32UTF8',
        'prefix' => '',
        // yajra assumes 11g otherwise and refuses JSON paths.
        'server_version' => '21c',
    ]);
} else {
    $capsule->addConnection([
        'driver' => 'mysql',
        'host' => '127.0.0.1',
        'port' => 13306,
        'database' => 'orm_test',
        'username' => 'orm',
        'password' => 'secret',
        'charset' => 'utf8mb4',
        'collation' => 'utf8mb4_unicode_ci',
    ]);
}
$capsule->setAsGlobal();
$capsule->bootEloquent();

class User extends Model
{
    protected $table = 'bench_users';

    protected $guarded = [];

    public function posts(): HasMany
    {
        return $this->hasMany(Post::class, 'user_id');
    }
}

class Post extends Model
{
    protected $table = 'bench_posts';

    protected $guarded = [];
}

class Event extends Model
{
    protected $table = 'bench_events';

    protected $guarded = [];

    protected $casts = ['payload' => 'array'];
}

// A /proc/self/status field such as "VmHWM:", in bytes.
function procStatus(string $key): int
{
    preg_match('/^'.$key.'\s+(\d+) kB/m', file_get_contents('/proc/self/status'), $m);

    return (int) ($m[1] ?? 0) * 1024;
}

// Runs $body a few times untimed, then $n times timed, one sample per run,
// and records the memory used during the timed runs.
function bench(array &$out, string $name, int $n, callable $body): void
{
    // Warm-up indices follow the timed ones, so updates never repeat a value.
    for ($i = $n; $i < $n + max(1, min(100, intdiv($n, 10))); $i++) {
        $body($i);
    }
    $ns = array_fill(0, $n, 0);
    gc_collect_cycles();
    memory_reset_peak_usage();
    $heap = memory_get_usage();
    // "5" resets VmHWM (peak RSS) to the current RSS.
    @file_put_contents('/proc/self/clear_refs', '5');
    for ($i = 0; $i < $n; $i++) {
        $start = hrtime(true);
        $body($i);
        $ns[$i] = hrtime(true) - $start;
    }
    $out[$name] = [
        'ns' => $ns,
        'heap_peak' => memory_get_peak_usage() - $heap,
        'rss_peak' => procStatus('VmHWM:'),
    ];
}

$out = [];
// Differs per process, so saves never find the values an earlier runner wrote.
$run = (int) (microtime(true) * 1e6);
$kinds = ['click', 'view', 'purchase', 'signup', 'logout'];
$userId = fn (int $i) => $i % 100000 + 1;
$age = fn (int $i) => 18 + $i % 60;
$users = fn (int $offset, int $limit) => User::orderBy('id')->offset($offset)->limit($limit)->get();
$eager = fn (int $offset, int $limit) => User::orderBy('id')->offset($offset)->limit($limit)->with('posts')->get();
$events = fn (int $limit) => Event::orderBy('id')->limit($limit)->get();
$bulkInsert = function (int $count) use ($age) {
    $ts = date('Y-m-d H:i:s');
    $rows = [];
    for ($j = 0; $j < $count; $j++) {
        $rows[] = ['name' => "Bulk $j", 'email' => "bulk$j@example.com", 'age' => $age($j), 'created_at' => $ts, 'updated_at' => $ts];
    }
    User::insert($rows);
};
$payload = fn (int $i) => [
    'type' => $kinds[$i % 5],
    'score' => $i,
    'tags' => ['alpha', 'beta', 'gamma'],
    'meta' => ['browser' => 'firefox', 'os' => 'linux', 'screen' => ['w' => 1920, 'h' => 1080]],
    'items' => [['sku' => 'A1', 'qty' => 2], ['sku' => 'B2', 'qty' => 1]],
];

bench($out, 'find_by_pk', 2000, fn ($i) => User::find($userId($i)));
bench($out, 'where_order_limit_20', 1000, fn ($i) => User::where('age', $age($i))->orderBy('id')->limit(20)->get());
bench($out, 'fetch_1000_rows', 100, fn ($i) => $users(0, 1000));
bench($out, 'fetch_10000_rows', 20, fn ($i) => $users(0, 10000));
bench($out, 'fetch_100000_rows', 5, fn ($i) => $users(0, 100000));
bench($out, 'eager_50_users_500_posts', 200, fn ($i) => $eager($i % 20 * 50, 50));
bench($out, 'eager_1000_users_10000_posts', 10, fn ($i) => $eager(0, 1000));
bench($out, 'json_fetch_1000', 100, fn ($i) => $events(1000));
bench($out, 'json_fetch_10000', 10, fn ($i) => $events(10000));
bench($out, 'json_where_limit_20', 1000, fn ($i) => Event::where('payload->type', $kinds[$i % 5])->orderBy('id')->limit(20)->get());

$loaded = $users(0, 1000);
bench($out, 'update_single', 1000, function ($i) use ($loaded, $run) {
    $user = $loaded[$i % 1000];
    $user->name = "Renamed $i $run";
    $user->save();
});
$loaded = $events(1000);
bench($out, 'json_update_single', 1000, function ($i) use ($loaded, $run) {
    $event = $loaded[$i % 1000];
    $payload = $event->payload;
    $payload['score'] = -$i;
    $payload['meta']['os'] = "os $i $run";
    $event->payload = $payload;
    $event->save();
});
bench($out, 'insert_single', 1000, fn ($i) => User::create(['name' => "New $i", 'email' => "new$i@example.com", 'age' => $age($i)]));
bench($out, 'json_insert_single', 1000, fn ($i) => Event::create(['user_id' => $userId($i), 'payload' => $payload($i)]));
bench($out, 'bulk_insert_1000', 20, fn ($i) => $bulkInsert(1000));
bench($out, 'bulk_insert_10000', 5, fn ($i) => $bulkInsert(10000));

file_put_contents($outPath, json_encode($out));
