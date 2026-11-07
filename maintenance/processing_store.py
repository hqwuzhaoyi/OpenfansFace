"""Durable processing state shared by the coordinator and the web UI."""
import json
import sqlite3
import time
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
 note TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS processing_stage ON tasks(stage, priority, is_new, created_at);
CREATE TABLE IF NOT EXISTS events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, at REAL NOT NULL, vid TEXT, kind TEXT NOT NULL, message TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS face_groups (id TEXT PRIMARY KEY, version TEXT NOT NULL, embedding BLOB NOT NULL);
'''


def connect(base):
    folder = Path(base) / 'processing'
    folder.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(folder / 'processing.db', timeout=15)
    con.row_factory = sqlite3.Row
    con.execute('PRAGMA busy_timeout=15000')
    con.executescript(SCHEMA)
    con.execute("INSERT OR IGNORE INTO settings VALUES ('paused','false')")
    con.commit()
    return con


def setting(con, key, fallback=None):
    row = con.execute('SELECT value FROM settings WHERE key=?', (key,)).fetchone()
    return json.loads(row[0]) if row else fallback


def set_setting(con, key, value):
    con.execute('INSERT INTO settings VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value',
                (key, json.dumps(value, ensure_ascii=False)))


def event(con, vid, kind, message):
    con.execute('INSERT INTO events(at,vid,kind,message) VALUES (?,?,?,?)', (time.time(), vid, kind, message))


def reconcile(con, items, batch_id, bootstrapped):
    """Fingerprint changes invalidate analysis, but retain the human note."""
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
            con.execute('UPDATE tasks SET stage=?,updated_at=? WHERE vid=?',
                        ('error' if before['manual'] else 'review' if ready else 'queued', stamp, vid))
        elif before['path'] != item['path'] or before['size'] != item['size'] or before['mtime_ns'] != item['mtime_ns']:
            con.execute('''UPDATE tasks SET path=?,name=?,size=?,mtime_ns=?,duration=?,stage='queued',retry_stage='text',
              text_attempts=0,face_attempts=0,next_retry=0,error=NULL,data='{}',manual=0,deferred=0,
              batch_id=?,updated_at=? WHERE vid=?''',
                        (item['path'], item['name'], item['size'], item['mtime_ns'], item.get('duration'), batch_id, stamp, vid))
    for row in con.execute("SELECT vid,stage FROM tasks WHERE stage!='excluded'").fetchall():
        if row['vid'] not in active:
            con.execute("UPDATE tasks SET stage='excluded',updated_at=? WHERE vid=?", (time.time(), row['vid']))
    return len(active)


def claim(con):
    con.execute('BEGIN IMMEDIATE')
    try:
        if setting(con, 'paused', False):
            con.commit()
            return None
        row = con.execute('''SELECT * FROM tasks WHERE stage IN ('queued','face_queued','retry') AND next_retry<=?
            ORDER BY priority DESC,is_new DESC,created_at DESC,vid LIMIT 1''', (time.time(),)).fetchone()
        if not row:
            con.commit()
            return None
        task = dict(row)
        stage = 'face' if task['stage'] == 'face_queued' else task['retry_stage'] if task['stage'] == 'retry' else 'text'
        field = 'text_attempts' if stage == 'text' else 'face_attempts'
        con.execute(f'UPDATE tasks SET stage=?,retry_stage=?,{field}={field}+1,updated_at=?,error=NULL WHERE vid=?',
                    (stage, stage, time.time(), task['vid']))
        event(con, task['vid'], 'started', '文字线索提取' if stage == 'text' else '人脸相似度分析')
        con.commit()
        task['stage'] = stage
        task[field] += 1
        return task
    except BaseException:
        con.rollback()
        raise


def finish(con, task, data, next_stage):
    result = con.execute('''UPDATE tasks SET data=?,stage=?,error=NULL,next_retry=0,updated_at=?
        WHERE vid=? AND stage=? AND size=? AND mtime_ns=?''',
                         (json.dumps(data, ensure_ascii=False), next_stage, time.time(), task['vid'], task['stage'], task['size'], task['mtime_ns']))
    if result.rowcount:
        event(con, task['vid'], 'ready' if next_stage == 'review' else 'stage_complete',
              '分析完成，等待人工确认' if next_stage == 'review' else '文字线索完成，等待人脸分析')
    con.commit()


def fail(con, task, error):
    attempts = task['text_attempts'] if task['stage'] == 'text' else task['face_attempts']
    stage = 'retry' if attempts < 3 else 'error'
    result = con.execute('''UPDATE tasks SET stage=?,retry_stage=?,error=?,next_retry=?,updated_at=?
        WHERE vid=? AND stage=? AND size=? AND mtime_ns=?''',
                         (stage, task['stage'], str(error)[:500], time.time() + min(300, 30 * attempts), time.time(),
                          task['vid'], task['stage'], task['size'], task['mtime_ns']))
    if result.rowcount:
        event(con, task['vid'], stage, str(error)[:500])
    con.commit()


def recover(con):
    # The coordinator holds an exclusive OS lock before calling this.
    con.execute("""UPDATE tasks SET stage=CASE WHEN
        (stage='text' AND text_attempts>=3) OR (stage='face' AND face_attempts>=3) THEN 'error' ELSE 'retry' END,
        next_retry=0,error='服务重启，中断阶段按重试上限恢复' WHERE stage IN ('text','face')""")
    con.commit()
