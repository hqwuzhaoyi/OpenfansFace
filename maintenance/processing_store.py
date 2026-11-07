"""Durable task leases, fixed discovery cohorts, and model-stage run history."""
import json
import sqlite3
import time
import uuid
from pathlib import Path

SCHEMA = '''
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks (
 vid TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
 size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, duration REAL,
 stage TEXT NOT NULL DEFAULT 'queued', retry_stage TEXT NOT NULL DEFAULT 'text',
 text_attempts INTEGER NOT NULL DEFAULT 0, face_attempts INTEGER NOT NULL DEFAULT 0,
 next_retry REAL NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 0,
 is_new INTEGER NOT NULL DEFAULT 0, batch_id TEXT NOT NULL,
 created_at REAL NOT NULL, updated_at REAL NOT NULL,
 error TEXT, manual INTEGER NOT NULL DEFAULT 0, deferred INTEGER NOT NULL DEFAULT 0,
 note TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '{}', run_id TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS processing_stage ON tasks(stage, priority, is_new, created_at);
CREATE TABLE IF NOT EXISTS events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, at REAL NOT NULL, vid TEXT, kind TEXT NOT NULL, message TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS face_groups (id TEXT PRIMARY KEY, version TEXT NOT NULL, embedding BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS batch_items (
 batch_id TEXT NOT NULL, vid TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL,
 size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, added_at REAL NOT NULL,
 text_done INTEGER NOT NULL DEFAULT 0, face_required INTEGER, face_done INTEGER NOT NULL DEFAULT 0,
 face_skipped INTEGER NOT NULL DEFAULT 0, completed INTEGER NOT NULL DEFAULT 0,
 completed_at REAL, excluded INTEGER NOT NULL DEFAULT 0, legacy INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(batch_id,vid)
);
CREATE TABLE IF NOT EXISTS processing_runs (
 id TEXT PRIMARY KEY, vid TEXT NOT NULL, name TEXT NOT NULL, batch_id TEXT NOT NULL,
 stage TEXT NOT NULL, started_at REAL NOT NULL, finished_at REAL, status TEXT NOT NULL,
 error TEXT, execution TEXT
);
CREATE INDEX IF NOT EXISTS processing_run_time ON processing_runs(started_at DESC);
'''


def connect(base):
    folder = Path(base) / 'processing'
    folder.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(folder / 'processing.db', timeout=15)
    con.row_factory = sqlite3.Row
    con.execute('PRAGMA busy_timeout=15000')
    con.executescript(SCHEMA)
    if 'run_id' not in {r[1] for r in con.execute('PRAGMA table_info(tasks)')}:
        con.execute("ALTER TABLE tasks ADD COLUMN run_id TEXT NOT NULL DEFAULT ''")
    for name, declaration in (('lease_expires_at', 'REAL NOT NULL DEFAULT 0'), ('coordinator_generation', "TEXT NOT NULL DEFAULT ''")):
        if name not in {r[1] for r in con.execute('PRAGMA table_info(tasks)')}:
            con.execute(f'ALTER TABLE tasks ADD COLUMN {name} {declaration}')
    if 'execution' not in {r[1] for r in con.execute('PRAGMA table_info(processing_runs)')}:
        con.execute('ALTER TABLE processing_runs ADD COLUMN execution TEXT')
    con.execute("INSERT OR IGNORE INTO settings VALUES ('paused','false')")
    con.execute("INSERT OR IGNORE INTO settings VALUES ('concurrency','1')")
    if not setting(con, 'batch_ledger_version', 0):
        for row in con.execute('SELECT * FROM tasks').fetchall():
            ensure_member(con, dict(row), legacy=True)
        set_setting(con, 'batch_ledger_version', 1)
    con.commit()
    return con


def setting(con, key, fallback=None):
    row = con.execute('SELECT value FROM settings WHERE key=?', (key,)).fetchone()
    return json.loads(row[0]) if row else fallback


def set_setting(con, key, value):
    con.execute('INSERT INTO settings VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value',
                (key, json.dumps(value, ensure_ascii=False)))


def effective_concurrency(con, capacity=1):
    value = setting(con, 'concurrency', 1)
    return min(max(1, capacity), max(1, value if type(value) is int else 1))


def event(con, vid, kind, message):
    con.execute('INSERT INTO events(at,vid,kind,message) VALUES (?,?,?,?)', (time.time(), vid, kind, message))


def evidence_flags(data):
    text_done = bool(data.get('text_complete'))
    skipped = bool(data.get('face_skipped'))
    return int(text_done), int(not skipped) if text_done else None, int(bool(data.get('face_complete'))), int(skipped), int(bool(data.get('ready')))


def ensure_member(con, task, legacy=False):
    try:
        data = json.loads(task.get('data', '{}'))
    except (ValueError, TypeError):
        data = {}
    flags = evidence_flags(data)
    con.execute('''INSERT OR IGNORE INTO batch_items
        (batch_id,vid,name,path,size,mtime_ns,added_at,text_done,face_required,face_done,face_skipped,completed,excluded,legacy)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)''',
        (task['batch_id'], task['vid'], task['name'], task['path'], task['size'], task['mtime_ns'], task['created_at'],
         *flags, int(task['stage'] == 'excluded'), int(legacy)))
    # Completed cohort evidence is historical; confirming/archiving or refreshing references cannot undo it.
    con.execute('UPDATE batch_items SET excluded=? WHERE batch_id=? AND vid=? AND completed=0',
                (int(task['stage'] == 'excluded'), task['batch_id'], task['vid']))


def reconcile(con, items, batch_id, bootstrapped):
    """Fingerprint changes start a new cohort entry; existing completion stays auditable."""
    current = {row['path']: dict(row) for row in con.execute('SELECT * FROM tasks')}
    by_vid = {row['vid']: row for row in current.values()}
    active = set()
    for item in items:
        before = current.get(item['path']) or by_vid.get(item['vid'])
        vid = before['vid'] if before else item['vid']
        active.add(vid)
        stamp = time.time()
        if not before:
            con.execute('''INSERT OR IGNORE INTO tasks
              (vid,path,name,size,mtime_ns,duration,batch_id,created_at,updated_at,is_new)
              VALUES (?,?,?,?,?,?,?,?,?,?)''',
                        (vid, item['path'], item['name'], item['size'], item['mtime_ns'], item.get('duration'), batch_id, stamp, stamp, int(bootstrapped)))
        elif before['stage'] == 'excluded' and before['path'] == item['path'] and before['size'] == item['size'] and before['mtime_ns'] == item['mtime_ns']:
            ready = json.loads(before['data']).get('ready')
            con.execute('UPDATE tasks SET stage=?,updated_at=?,run_id=? WHERE vid=?',
                        ('error' if before['manual'] else 'review' if ready else 'queued', stamp, '', vid))
        elif before['path'] != item['path'] or before['size'] != item['size'] or before['mtime_ns'] != item['mtime_ns']:
            con.execute('UPDATE batch_items SET excluded=1 WHERE batch_id=? AND vid=? AND completed=0', (before['batch_id'], vid))
            con.execute('''UPDATE tasks SET path=?,name=?,size=?,mtime_ns=?,duration=?,stage='queued',retry_stage='text',
              text_attempts=0,face_attempts=0,next_retry=0,error=NULL,data='{}',manual=0,deferred=0,run_id='',
              batch_id=?,updated_at=? WHERE vid=?''',
                        (item['path'], item['name'], item['size'], item['mtime_ns'], item.get('duration'), batch_id, stamp, vid))
        row = con.execute('SELECT * FROM tasks WHERE vid=?', (vid,)).fetchone()
        if row:
            ensure_member(con, dict(row))
    for row in con.execute("SELECT vid,batch_id,stage FROM tasks WHERE stage!='excluded'").fetchall():
        if row['vid'] not in active:
            con.execute("UPDATE tasks SET stage='excluded',updated_at=?,run_id='' WHERE vid=?", (time.time(), row['vid']))
            con.execute('UPDATE batch_items SET excluded=1 WHERE batch_id=? AND vid=? AND completed=0', (row['batch_id'], row['vid']))
    return len(active)


def claim(con, capacity=1):
    con.execute('BEGIN IMMEDIATE')
    try:
        if setting(con, 'paused', False) or con.execute("SELECT COUNT(*) FROM tasks WHERE stage IN ('text','face')").fetchone()[0] >= effective_concurrency(con, capacity):
            con.commit()
            return None
        row = con.execute('''SELECT * FROM tasks WHERE stage IN ('queued','face_queued','retry') AND manual=0 AND next_retry<=?
            ORDER BY priority DESC,is_new DESC,created_at DESC,vid LIMIT 1''', (time.time(),)).fetchone()
        if not row:
            con.commit()
            return None
        task = dict(row)
        stage = 'face' if task['stage'] == 'face_queued' else task['retry_stage'] if task['stage'] == 'retry' else 'text'
        field = 'text_attempts' if stage == 'text' else 'face_attempts'
        run_id, at = str(uuid.uuid4()), time.time()
        generation = setting(con, 'coordinator_generation', 'unstarted')
        expiry = at + 660
        con.execute(f'UPDATE tasks SET stage=?,retry_stage=?,{field}={field}+1,updated_at=?,error=NULL,run_id=?,lease_expires_at=?,coordinator_generation=? WHERE vid=?',
                    (stage, stage, at, run_id, expiry, generation, task['vid']))
        con.execute('INSERT INTO processing_runs(id,vid,name,batch_id,stage,started_at,status) VALUES (?,?,?,?,?,?,?)',
                    (run_id, task['vid'], task['name'], task['batch_id'], stage, at, 'running'))
        event(con, task['vid'], 'started', '文字线索提取' if stage == 'text' else '人脸相似度分析')
        con.commit()
        task.update(stage=stage, run_id=run_id, lease_expires_at=expiry, coordinator_generation=generation)
        task[field] += 1
        return task
    except BaseException:
        con.rollback()
        raise


def guard(task):
    return task['vid'], task['stage'], task['path'], task['size'], task['mtime_ns'], task.get('run_id', '')


GUARD = 'vid=? AND stage=? AND path=? AND size=? AND mtime_ns=? AND run_id=? AND manual=0'


def close_run(con, task, status, error=None):
    con.execute("UPDATE processing_runs SET status=?,error=?,finished_at=? WHERE id=? AND status='running'",
                (status, str(error)[:500] if error else None, time.time(), task.get('run_id', '')))


def finish(con, task, data, next_stage):
    result = con.execute(f'UPDATE tasks SET data=?,stage=?,error=NULL,next_retry=0,updated_at=? WHERE {GUARD}',
                         (json.dumps(data, ensure_ascii=False), next_stage, time.time(), *guard(task)))
    if result.rowcount:
        flags = evidence_flags(data)
        con.execute('''UPDATE batch_items SET text_done=?,face_required=?,face_done=?,face_skipped=?,completed=?,
            completed_at=CASE WHEN ?=1 THEN ? ELSE NULL END WHERE batch_id=? AND vid=? AND completed=0''',
                    (*flags, flags[-1], time.time(), task['batch_id'], task['vid']))
        event(con, task['vid'], 'ready' if next_stage == 'review' else 'stage_complete',
              '分析完成，等待人工确认' if next_stage == 'review' else '文字线索完成，等待人脸分析')
    close_run(con, task, 'done' if result.rowcount else 'superseded')
    con.commit()


def fail(con, task, error):
    attempts = task['text_attempts'] if task['stage'] == 'text' else task['face_attempts']
    stage = 'retry' if attempts < 3 else 'error'
    result = con.execute(f'UPDATE tasks SET stage=?,retry_stage=?,error=?,next_retry=?,updated_at=? WHERE {GUARD}',
                         (stage, task['stage'], str(error)[:500], time.time() + min(300, 30 * attempts), time.time(), *guard(task)))
    if result.rowcount:
        event(con, task['vid'], stage, str(error)[:500])
    close_run(con, task, stage if result.rowcount else 'superseded', error)
    con.commit()


def release_task(con, task, reason, excluded=False):
    field = 'text_attempts' if task['stage'] == 'text' else 'face_attempts'
    result = con.execute(f'''UPDATE tasks SET stage=?,next_retry=0,{field}=MAX(0,{field}-1),error=?,updated_at=?
        WHERE {GUARD}''', ('excluded' if excluded else 'retry', None if excluded else reason, time.time(), *guard(task)))
    if result.rowcount and excluded:
        con.execute('UPDATE batch_items SET excluded=1 WHERE batch_id=? AND vid=? AND completed=0', (task['batch_id'], task['vid']))
    close_run(con, task, ('cancelled' if excluded else 'waiting') if result.rowcount else 'superseded', reason)
    con.commit()


def recover(con):
    set_setting(con, 'coordinator_generation', str(uuid.uuid4()))
    # Only call with the exclusive coordinator lock and no live stage futures.
    con.execute("UPDATE processing_runs SET status='interrupted',finished_at=?,error='协调器重启或阶段意外中断' WHERE status='running'", (time.time(),))
    con.execute("""UPDATE tasks SET stage=CASE WHEN
        (stage='text' AND text_attempts>=3) OR (stage='face' AND face_attempts>=3) THEN 'error' ELSE 'retry' END,
        next_retry=0,error='服务重启，中断阶段按重试上限恢复',run_id='' WHERE stage IN ('text','face')""")
    con.commit()
